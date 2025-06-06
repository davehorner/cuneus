use cuneus::{Core, ShaderManager,UniformProvider, UniformBinding,RenderKit,TextureManager,ShaderHotReload,ShaderControls,AtomicBuffer};
use winit::event::WindowEvent;
use cuneus::ShaderApp;
use cuneus::Renderer;
use cuneus::create_feedback_texture_pair;
use cuneus::ExportManager;
use std::path::PathBuf;
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct FeedbackParams {
    decay: f32,
    speed: f32,
    intensity: f32,
    scale: f32,
    rotation_x: f32,
    rotation_y: f32,
    rotation_z: f32,
    rotation_speed: f32,
    attractor_a: f32,
    attractor_b: f32,
    attractor_c: f32,
    attractor_d: f32,
    attractor_animate_amount: f32,
}
impl UniformProvider for FeedbackParams {
    fn as_bytes(&self) -> &[u8] {
        bytemuck::bytes_of(self)
    }
}
struct Clifford {
    base: RenderKit,
    renderer_pass2: Renderer,
    params_uniform: UniformBinding<FeedbackParams>,
    texture_a: Option<TextureManager>,
    texture_b: Option<TextureManager>,
    frame_count: u32,
    hot_reload: ShaderHotReload,
    texture_bind_group_layout: wgpu::BindGroupLayout,
    time_bind_group_layout: wgpu::BindGroupLayout,
    params_bind_group_layout: wgpu::BindGroupLayout,
    atomic_buffer: AtomicBuffer,
    atomic_bind_group_layout: wgpu::BindGroupLayout,
}
impl Clifford {
    fn capture_frame(&mut self, core: &Core, time: f32) -> Result<Vec<u8>, wgpu::SurfaceError> {
        let settings = self.base.export_manager.settings();
        let (capture_texture, output_buffer) = self.base.create_capture_texture(
            &core.device,
            settings.width,
            settings.height
        );
        let align = 256;
        let unpadded_bytes_per_row = settings.width * 4;
        let padding = (align - unpadded_bytes_per_row % align) % align;
        let padded_bytes_per_row = unpadded_bytes_per_row + padding;
        let capture_view = capture_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = core.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Capture Encoder"),
        });
        self.base.time_uniform.data.time = time;
        self.base.time_uniform.update(&core.queue);
        {
            self.atomic_buffer.clear(&core.queue);
            let mut render_pass = Renderer::begin_render_pass(
                &mut encoder,
                &capture_view,
                wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                Some("Capture Pass"),
            );
            render_pass.set_pipeline(&self.renderer_pass2.render_pipeline);
            render_pass.set_vertex_buffer(0, self.renderer_pass2.vertex_buffer.slice(..));
            if let Some(texture) = &self.texture_a {
                render_pass.set_bind_group(0, &texture.bind_group, &[]);
            }
            render_pass.set_bind_group(1, &self.base.time_uniform.bind_group, &[]);
            render_pass.set_bind_group(2, &self.params_uniform.bind_group, &[]);
            render_pass.set_bind_group(3, &self.atomic_buffer.bind_group, &[]);
            render_pass.draw(0..4, 0..1);
        }

        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &capture_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &output_buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(settings.height),
                },
            },
            wgpu::Extent3d {
                width: settings.width,
                height: settings.height,
                depth_or_array_layers: 1,
            },
        );
        core.queue.submit(Some(encoder.finish()));
        let buffer_slice = output_buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            tx.send(result).unwrap();
        });
        core.device.poll(wgpu::Maintain::Wait);
        rx.recv().unwrap().unwrap();
        let padded_data = buffer_slice.get_mapped_range().to_vec();
        let mut unpadded_data = Vec::with_capacity((settings.width * settings.height * 4) as usize);
        for chunk in padded_data.chunks(padded_bytes_per_row as usize) {
            unpadded_data.extend_from_slice(&chunk[..unpadded_bytes_per_row as usize]);
        }
        Ok(unpadded_data)
    }

    fn handle_export(&mut self, core: &Core) {
        if let Some((frame, time)) = self.base.export_manager.try_get_next_frame() {
            if let Ok(data) = self.capture_frame(core, time) {
                let settings = self.base.export_manager.settings();
                if let Err(e) = cuneus::save_frame(data, frame, settings) {
                    eprintln!("Error saving frame: {:?}", e);
                }
            }
        } else {
            self.base.export_manager.complete_export();
        }
    }
}


impl ShaderManager for Clifford {
    fn init(core: &Core) -> Self {
        let texture_bind_group_layout = core.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
            label: Some("texture_bind_group_layout"),
        });

        let time_bind_group_layout = core.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
            label: Some("time_bind_group_layout"),
        });
        let params_bind_group_layout = core.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
            label: Some("params_bind_group_layout"),
        });
        let atomic_bind_group_layout = core.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
            label: Some("atomic_bind_group_layout"),
        });

        let buffer_size = core.config.width * core.config.height;
        let atomic_buffer = AtomicBuffer::new(
            &core.device,
            buffer_size,
            &atomic_bind_group_layout,
        );
        let params_uniform = UniformBinding::new(
            &core.device,
            "Params Uniform",
            FeedbackParams {
                decay: 0.9,
                speed: 1.0,
                intensity: 1.0,
                scale: 1.0,
                rotation_x: 0.0,
                rotation_y: 0.0,
                rotation_z: 0.0,
                rotation_speed: 0.15,
                attractor_a: 1.7,
                attractor_b: 1.7,
                attractor_c: 0.6,
                attractor_d: 1.2,
                attractor_animate_amount: 1.0,
            },
            &params_bind_group_layout,
            0,
        );
        let (texture_a, texture_b) = create_feedback_texture_pair(
            core,
            core.config.width,
            core.config.height,
            &texture_bind_group_layout,
        );
        let vs_module = core.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Vertex Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../shaders/vertex.wgsl").into()),
        });
        let fs_module = core.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Fragment Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../shaders/clifford.wgsl").into()),
        });
        let pipeline_layout = core.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Render Pipeline Layout"),
            bind_group_layouts: &[
                &texture_bind_group_layout,
                &time_bind_group_layout,
                &params_bind_group_layout,
                &atomic_bind_group_layout,
            ],
            push_constant_ranges: &[],
        });
        let shader_paths = vec![
            PathBuf::from("shaders/vertex.wgsl"),
            PathBuf::from("shaders/clifford.wgsl"),
        ];
        let hot_reload = ShaderHotReload::new(
            core.device.clone(),
            shader_paths,
            vs_module,
            fs_module,
        ).expect("Failed to initialize hot reload");
        let renderer_pass2 = Renderer::new(
            &core.device,
            &hot_reload.vs_module,
            &hot_reload.fs_module,
            core.config.format,
            &pipeline_layout,
            Some("fs_pass2"),
        );
        let base = RenderKit::new(
            core,
            include_str!("../../shaders/vertex.wgsl"),
            include_str!("../../shaders/clifford.wgsl"),
            &[
                &texture_bind_group_layout,
                &time_bind_group_layout,
                &params_bind_group_layout,
                &atomic_bind_group_layout,
            ],
            Some("fs_pass1"),
        );
        Self {
            base,
            renderer_pass2,
            params_uniform,
            texture_a: Some(texture_a),
            texture_b: Some(texture_b),
            frame_count: 0,
            hot_reload,
            texture_bind_group_layout,
            time_bind_group_layout,
            params_bind_group_layout,
            atomic_buffer,
            atomic_bind_group_layout,
        }
    }
    fn update(&mut self, core: &Core) {
        if let Some((new_vs, new_fs)) = self.hot_reload.check_and_reload() {
            println!("Reloading shaders at time: {:.2}s", self.base.start_time.elapsed().as_secs_f32());
            let pipeline_layout = core.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Render Pipeline Layout"),
                bind_group_layouts: &[
                    &self.texture_bind_group_layout,
                    &self.time_bind_group_layout,
                    &self.params_bind_group_layout,
                    &self.atomic_bind_group_layout,
                ],
                push_constant_ranges: &[],
            });
            self.renderer_pass2 = Renderer::new(
                &core.device,
                new_vs,
                new_fs,
                core.config.format,
                &pipeline_layout,
                Some("fs_pass2"),
            );
    
            self.base.renderer = Renderer::new(
                &core.device,
                new_vs,
                new_fs,
                core.config.format,
                &pipeline_layout,
                Some("fs_pass1"),
            );
        }
        if self.base.export_manager.is_exporting() {
            self.handle_export(core);
        }
        self.base.fps_tracker.update();
    }
    fn render(&mut self, core: &Core) -> Result<(), wgpu::SurfaceError> {
        let output = core.surface.get_current_texture()?;
        let view = output.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = core.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Render Encoder"),
        });
        let mut params = self.params_uniform.data;
        let mut changed = false;
        let mut should_start_export = false;
        let mut export_request = self.base.export_manager.get_ui_request();
        let mut controls_request = self.base.controls.get_ui_request(
            &self.base.start_time,
            &core.size
        );
        controls_request.current_fps = Some(self.base.fps_tracker.fps());
        let full_output = if self.base.key_handler.show_ui {
            self.base.render_ui(core, |ctx| {
                ctx.style_mut(|style| {
                    style.visuals.window_fill = egui::Color32::from_rgba_premultiplied(0, 0, 0, 180);
                });
                egui::Window::new("Settings").show(ctx, |ui| {
                    ui.group(|ui| {
                        ui.heading("Visual");
                        changed |= ui.add(egui::Slider::new(&mut params.decay, 0.1..=1.0).text("Decay")).changed();
                        changed |= ui.add(egui::Slider::new(&mut params.intensity, 0.1..=3.99).text("Intensity")).changed();
                        changed |= ui.add(egui::Slider::new(&mut params.speed, 0.1..=4.0).text("Speed")).changed();
                        changed |= ui.add(egui::Slider::new(&mut params.scale, 0.1..=4.0).text("Scale")).changed();
                    });
        
                    ui.add_space(10.0);
        
                    ui.group(|ui| {
                        ui.heading("Rot");
                        changed |= ui.add(egui::Slider::new(&mut params.rotation_x, -3.14..=3.14).text("X")).changed();
                        changed |= ui.add(egui::Slider::new(&mut params.rotation_y, -3.14..=3.14).text("Y")).changed();
                        changed |= ui.add(egui::Slider::new(&mut params.rotation_z, -3.14..=3.14).text("Z")).changed();
                        changed |= ui.add(egui::Slider::new(&mut params.rotation_speed, 0.0..=1.0).text("t")).changed();
                    });
        
                    ui.add_space(10.0);
        
                    ui.group(|ui| {
                        ui.heading("Attractor");
                        changed |= ui.add(egui::Slider::new(&mut params.attractor_a, 0.0..=3.0).text("a")).changed();
                        changed |= ui.add(egui::Slider::new(&mut params.attractor_b, 0.0..=3.0).text("b")).changed();
                        changed |= ui.add(egui::Slider::new(&mut params.attractor_c, 0.0..=3.0).text("c")).changed();
                        changed |= ui.add(egui::Slider::new(&mut params.attractor_d, 0.0..=3.0).text("d")).changed();
                        changed |= ui.add(egui::Slider::new(&mut params.attractor_animate_amount, 0.0..=2.0).text("Anim")).changed();
                    });
        
                    ui.separator();
                    ShaderControls::render_controls_widget(ui, &mut controls_request);
                    ui.separator();
                    should_start_export = ExportManager::render_export_ui_widget(ui, &mut export_request);
              
                });
            })
        } else {
            self.base.render_ui(core, |_ctx| {})
        };
        self.base.export_manager.apply_ui_request(export_request);
        if controls_request.should_clear_buffers {
            let (texture_a, texture_b) = create_feedback_texture_pair(
                core,
                core.config.width,
                core.config.height,
                &self.texture_bind_group_layout,
            );
            self.texture_a = Some(texture_a);
            self.texture_b = Some(texture_b);
        }
        self.base.apply_control_request(controls_request);
        let current_time = self.base.controls.get_time(&self.base.start_time);
        self.base.time_uniform.data.time = current_time;
        self.base.time_uniform.update(&core.queue);
        if changed {
            self.params_uniform.data = params;
            self.params_uniform.update(&core.queue);
        }
        if should_start_export {
            self.base.export_manager.start_export();
        }
        if let (Some(ref texture_a), Some(ref texture_b)) = (&self.texture_a, &self.texture_b) {
            let (source_texture, target_texture) = if self.frame_count % 2 == 0 {
                (texture_b, texture_a)
            } else {
                (texture_a, texture_b)
            };
            
            // First render pass
{
                self.atomic_buffer.clear(&core.queue);
                let mut render_pass = Renderer::begin_render_pass(
                    &mut encoder,
                    &target_texture.view,
                    wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    Some("Feedback Pass"),
                );
                render_pass.set_pipeline(&self.base.renderer.render_pipeline);
                render_pass.set_vertex_buffer(0, self.base.renderer.vertex_buffer.slice(..));
                render_pass.set_bind_group(0, &source_texture.bind_group, &[]);
                render_pass.set_bind_group(1, &self.base.time_uniform.bind_group, &[]);
                render_pass.set_bind_group(2, &self.params_uniform.bind_group, &[]);
                render_pass.set_bind_group(3, &self.atomic_buffer.bind_group, &[]);
                render_pass.draw(0..4, 0..1);
            }
    
            // Second render pass
            {   self.atomic_buffer.clear(&core.queue);
                let mut render_pass = Renderer::begin_render_pass(
                    &mut encoder,
                    &view,
                    wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    Some("Display Pass"),
                );
                render_pass.set_pipeline(&self.renderer_pass2.render_pipeline);
                render_pass.set_vertex_buffer(0, self.renderer_pass2.vertex_buffer.slice(..));
                render_pass.set_bind_group(0, &target_texture.bind_group, &[]);
                render_pass.set_bind_group(1, &self.base.time_uniform.bind_group, &[]);
                render_pass.set_bind_group(2, &self.params_uniform.bind_group, &[]);
                render_pass.set_bind_group(3, &self.atomic_buffer.bind_group, &[]);
                render_pass.draw(0..4, 0..1);
            }
            self.frame_count = self.frame_count.wrapping_add(1);
        }
        self.base.handle_render_output(core, &view, full_output, &mut encoder);
        encoder.insert_debug_marker("Transition to Present");
        core.queue.submit(Some(encoder.finish()));
        output.present();
    
        Ok(())
    }
    fn handle_input(&mut self, core: &Core, event: &WindowEvent) -> bool {
        if self.base.egui_state.on_window_event(core.window(), event).consumed {
            return true;
        }
        if let WindowEvent::KeyboardInput { event, .. } = event {
            return self.base.key_handler.handle_keyboard_input(core.window(), event);
        }
        false
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    let (app, event_loop) = ShaderApp::new("Clifford", 800, 600);
    app.run(event_loop, |core| {
        Clifford::init(core)
    })
}
