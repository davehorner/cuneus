use cuneus::{Core,Renderer,ShaderApp, ShaderManager, UniformProvider, UniformBinding, RenderKit, ExportManager,ShaderHotReload,ShaderControls};
use winit::event::*;
use std::path::PathBuf;
// Remove or fix this import if the media module does not exist or is not needed
// use crate::media::ParamsKind;
// Pipeline layout kind for each shader
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PipelineLayoutKind {
    Texture, // For shaders that sample from a texture (e.g., video frame)
    Buffer,  // For shaders that use a storage or uniform buffer
    // Add more as needed
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ParamsKind {
    Payme,
    Droste,
    Spiral,
    Voronoi,
    GaborNoise,
    Sinh,
    // Add more as needed for each unique params struct
}

// List of video shader segments and their durations (in seconds)
pub struct ShaderSegment {
    pub path: &'static str,
    pub duration: f32,
    pub layout: PipelineLayoutKind,
    pub params_kind: ParamsKind,
}

// List of all video WGSL shaders, their durations, and required pipeline layout
pub const VIDEO_SHADER_SEGMENTS: &[ShaderSegment] = &[
    ShaderSegment { path: "shaders/payme.wgsl", duration: 4.0, layout: PipelineLayoutKind::Texture, params_kind: ParamsKind::Payme },
    // ShaderSegment { path: "shaders/audiovis.wgsl", duration: 10.0, layout: PipelineLayoutKind::Texture, params_kind: ParamsKind::Payme },
    // ShaderSegment { path: "shaders/asahi.wgsl", duration: 4.0, layout: PipelineLayoutKind::Texture, params_kind: ParamsKind::Asahi },
    // ShaderSegment { path: "shaders/blit.wgsl", duration: 4.0, layout: PipelineLayoutKind::Texture, params_kind: ParamsKind::Payme },
    ShaderSegment { path: "shaders/droste.wgsl", duration: 4.0, layout: PipelineLayoutKind::Texture, params_kind: ParamsKind::Droste },
    ShaderSegment { path: "shaders/spiral.wgsl", duration: 4.0, layout: PipelineLayoutKind::Texture, params_kind: ParamsKind::Spiral },
    ShaderSegment { path: "shaders/voronoi.wgsl", duration: 4.0, layout: PipelineLayoutKind::Texture, params_kind: ParamsKind::Voronoi },
    ShaderSegment { path: "shaders/gabornoise.wgsl", duration: 4.0, layout: PipelineLayoutKind::Texture, params_kind: ParamsKind::GaborNoise },
    ShaderSegment { path: "shaders/sinh.wgsl", duration: 4.0, layout: PipelineLayoutKind::Buffer, params_kind: ParamsKind::Sinh },
    // ...add more as needed
];
// Define all params structs
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PaymeParams {
    pub branches: f32,
    pub scale: f32,
    pub time_scale: f32,
    pub rotation: f32,
    pub zoom: f32,
    pub offset_x: f32,
    pub offset_y: f32,
    pub iterations: f32,
    pub smoothing: f32,
    pub use_animation: f32,
}

// Repeat for each unique params struct:
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct DrosteParams {
    // ...fields matching droste.wgsl Params...
}
// ...etc for SpiralParams, VoronoiParams, GaborNoiseParams, SinhParams...

impl UniformProvider for PaymeParams {
    fn as_bytes(&self) -> &[u8] {
        bytemuck::bytes_of(self)
    }
}

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GaborNoiseParams {
    pub width: f32,
    pub height: f32,
    pub steps: f32,
    pub _pad1: f32,
    pub kernel_size: f32,
    pub num_kernels: f32,
    pub frequency: f32,
    pub frequency_var: f32,
    pub seed: f32,
    pub animation_speed: f32,
    pub gamma: f32,
    pub _pad2: f32,
}

impl UniformProvider for GaborNoiseParams {
    fn as_bytes(&self) -> &[u8] {
        bytemuck::bytes_of(self)
    }
}

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SinhParams {
    pub color1: [f32; 3],
    pub pad1: f32,
    pub gradient_color: [f32; 3],
    pub _pad2: f32,
    pub c_value_max: f32,
    pub iterations: i32,
    pub aa_level: i32,
    pub _pad3: f32,
}

impl UniformProvider for SinhParams {
    fn as_bytes(&self) -> &[u8] {
        bytemuck::bytes_of(self)
    }
}

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct TimeUniform {
    pub time: f32,
    pub _padding: [f32; 3], // Pad to 16 bytes for WGSL alignment
}

impl UniformProvider for TimeUniform {
    fn as_bytes(&self) -> &[u8] {
        bytemuck::bytes_of(self)
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    cuneus::gst::init()?;
    env_logger::init();
    let (app, event_loop) = ShaderApp::new("PayMe", 800, 600);
    app.run(event_loop, |core| {
        PayMe::init(core)
    })
}

struct PayMe {
    base: RenderKit,
    params_uniform: UniformBinding<PaymeParams>,
    gabornoise_params: UniformBinding<GaborNoiseParams>,
    sinh_params: UniformBinding<SinhParams>,
    hot_reload: ShaderHotReload,
    texture_bind_group_layout: wgpu::BindGroupLayout,
    time_bind_group_layout: wgpu::BindGroupLayout,
    resolution_bind_group_layout: wgpu::BindGroupLayout,
    params_bind_group_layout: wgpu::BindGroupLayout,
    buffer_bind_group_layout: wgpu::BindGroupLayout,
    current_shader_index: usize,
}

impl PayMe {
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
        self.base.resolution_uniform.data.dimensions = [settings.width as f32, settings.height as f32];
        self.base.resolution_uniform.update(&core.queue);
        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Capture Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &capture_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            render_pass.set_pipeline(&self.base.renderer.render_pipeline);
            render_pass.set_vertex_buffer(0, self.base.renderer.vertex_buffer.slice(..));
            if self.base.using_video_texture {
                if let Some(video_manager) = &self.base.video_texture_manager {
                    render_pass.set_bind_group(0, &video_manager.texture_manager().bind_group, &[]);
                }
            } else if let Some(texture_manager) = &self.base.texture_manager {
                render_pass.set_bind_group(0, &texture_manager.bind_group, &[]);
            }
            // Time (group 1)
            render_pass.set_bind_group(1, &self.base.time_uniform.bind_group, &[]);
            // Params (group 2)
            render_pass.set_bind_group(2, &self.params_uniform.bind_group, &[]);
            // Resolution (group 3)
            render_pass.set_bind_group(3, &self.base.resolution_uniform.bind_group, &[]);
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

    fn get_active_shader_index(elapsed: f32) -> usize {
        let mut t = 0.0;
        for (i, seg) in VIDEO_SHADER_SEGMENTS.iter().enumerate() {
            t += seg.duration;
            if elapsed < t {
                return i;
            }
        }
        0 // Loop back to the first shader
    }
}
impl ShaderManager for PayMe {
    fn init(core: &cuneus::Core) -> Self {
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
        let resolution_bind_group_layout = core.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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
            label: Some("resolution_bind_group_layout"),
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
        let params_uniform = UniformBinding::new(
            &core.device,
            "Params Uniform",
            PaymeParams {
                branches: 1.0,
                scale: 0.5,
                time_scale: 1.0,
                rotation: 0.0,
                zoom: 1.0,
                offset_x: 0.0,
                offset_y: 0.0,
                iterations: 1.0,
                smoothing: 0.5,
                use_animation: 1.0,
            },
            &params_bind_group_layout,
            0,
        );
        let gabornoise_params = UniformBinding::new(
            &core.device,
            "GaborNoise Params Uniform",
            GaborNoiseParams {
                width: 512.0,
                height: 512.0,
                steps: 8.0,
                _pad1: 0.0,
                kernel_size: 5.0,
                num_kernels: 16.0,
                frequency: 0.5,
                frequency_var: 0.2,
                seed: 42.0,
                animation_speed: 1.0,
                gamma: 1.0,
                _pad2: 0.0,
            },
            &params_bind_group_layout,
            0,
        );
        let sinh_params = UniformBinding::new(
            &core.device,
            "Sinh Params Uniform",
            SinhParams {
                color1: [1.0, 0.5, 0.2],
                pad1: 0.0,
                gradient_color: [0.2, 0.8, 1.0],
                _pad2: 0.0,
                c_value_max: 2.5,
                iterations: 32,
                aa_level: 2,
                _pad3: 0.0,
            },
            &params_bind_group_layout,
            0,
        );
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
        let buffer_bind_group_layout = core.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform, // CHANGED from Storage to Uniform
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
            label: Some("buffer_bind_group_layout"),
        });
        let bind_group_layouts = vec![
            &texture_bind_group_layout,    // group 0
            &time_bind_group_layout,       // group 1 
            &params_bind_group_layout,     // group 2
            &resolution_bind_group_layout, // group 3
        ];
        let vs_module = core.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Vertex Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../shaders/vertex.wgsl").into()),
        });

        let fs_module = core.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Fragment Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../shaders/PayMe.wgsl").into()),
        });

        let shader_paths = vec![
            PathBuf::from("shaders/vertex.wgsl"),
            PathBuf::from("shaders/PayMe.wgsl"),
        ];

        let hot_reload = ShaderHotReload::new(
            core.device.clone(),
            shader_paths,
            vs_module,
            fs_module,
        ).expect("Failed to initialize hot reload");
        let base = RenderKit::new(
            core,
            include_str!("../../shaders/vertex.wgsl"),
            include_str!("../../shaders/payme.wgsl"),
            &bind_group_layouts,
            None,
        );
        Self {
            base,
            params_uniform,
            gabornoise_params,
            sinh_params,
            hot_reload,
            texture_bind_group_layout,
            time_bind_group_layout,
            resolution_bind_group_layout,
            params_bind_group_layout,
            buffer_bind_group_layout,
            current_shader_index: 0,
        }
    }

    fn update(&mut self, core: &Core) {
        if let Some((new_vs, new_fs)) = self.hot_reload.check_and_reload() {
            println!("Reloading shaders at time: {:.2}s", self.base.start_time.elapsed().as_secs_f32());
            let pipeline_layout = core.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Render Pipeline Layout"),
                bind_group_layouts: &[
                    &self.texture_bind_group_layout,    // group 0
                    &self.time_bind_group_layout,       // group 1
                    &self.resolution_bind_group_layout, // group 2
                    &self.params_bind_group_layout,     // group 3
                ],
                push_constant_ranges: &[],
            });
            self.base.renderer = Renderer::new(
                &core.device,
                new_vs,
                new_fs,
                core.config.format,
                &pipeline_layout,
                None,
            );
        }
        if self.base.export_manager.is_exporting() {
            self.handle_export(core);
        }
        self.base.fps_tracker.update();
    }
    
    fn render(&mut self, core: &Core) -> Result<(), wgpu::SurfaceError> {
        // --- Shader switching logic ---
        let elapsed = self.base.start_time.elapsed().as_secs_f32();
        let shader_index = Self::get_active_shader_index(elapsed);
        if shader_index != self.current_shader_index {
            let shader_segment = &VIDEO_SHADER_SEGMENTS[shader_index];
            let shader_path = shader_segment.path;
            let layout_kind = shader_segment.layout;
            println!("[SHADER] Switching to: {} (layout: {:?})", shader_path, layout_kind);
            match std::fs::read_to_string(shader_path) {
                Ok(shader_source) => {
                    println!("[SHADER] Source length: {} bytes", shader_source.len());
                    for (i, line) in shader_source.lines().take(5).enumerate() {
                        println!("[SHADER] {}: {}", i + 1, line);
                    }
                    let fs_module = core.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                        label: Some("Fragment Shader"),
                        source: wgpu::ShaderSource::Wgsl(shader_source.into()),
                    });
                    let vs_module = core.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                        label: Some("Vertex Shader"),
                        source: wgpu::ShaderSource::Wgsl(include_str!("../../shaders/vertex.wgsl").into()),
                    });
                    // Select bind group layouts based on layout_kind
                    let bind_group_layouts: Vec<&wgpu::BindGroupLayout> = match layout_kind {
                        PipelineLayoutKind::Texture => vec![
                            &self.texture_bind_group_layout,    // group 0
                            &self.time_bind_group_layout,       // group 1
                            &self.resolution_bind_group_layout, // group 2
                            &self.params_bind_group_layout,     // group 3
                        ],
                        PipelineLayoutKind::Buffer => {
                            // TODO: Add buffer_bind_group_layout to PayMe struct and initialize in init()
                            println!("[SHADER] Buffer layout requested but not implemented!");
                            vec![
                                &self.buffer_bind_group_layout,    // group 0 (buffer)
                                &self.time_bind_group_layout,
                                &self.resolution_bind_group_layout,
                                &self.params_bind_group_layout,
                            ]
                        }
                    };
                    let pipeline_layout = core.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                        label: Some("Render Pipeline Layout"),
                        bind_group_layouts: &bind_group_layouts,
                        push_constant_ranges: &[],
                    });
                    self.base.renderer = Renderer::new(
                        &core.device,
                        &vs_module,
                        &fs_module,
                        core.config.format,
                        &pipeline_layout,
                        None,
                    );
                    self.current_shader_index = shader_index;
                    println!("[SHADER] Switched to {}", shader_path);
                }
                Err(e) => {
                    println!("[SHADER] Failed to read WGSL: {} (error: {})", shader_path, e);
                }
            }
        }
        let output = core.surface.get_current_texture()?;
        let view = output.texture.create_view(&wgpu::TextureViewDescriptor::default());
        // Update video texture if one is loaded
        if self.base.using_video_texture {
            self.base.update_video_texture(core, &core.queue);
        }
        let mut params = self.params_uniform.data;
        let mut changed = false;
        let mut should_start_export = false;
        let mut export_request = self.base.export_manager.get_ui_request();
        let mut controls_request = self.base.controls.get_ui_request(
            &self.base.start_time,
            &core.size
        );
        // Extract all necessary state BEFORE rendering the UI.
        // also store actions to be performed after UI rendering. these are mostly due to fighting borrow checker :-(
        let using_video_texture = self.base.using_video_texture;
        let using_hdri_texture = self.base.using_hdri_texture;
        let video_info = self.base.get_video_info();
        let hdri_info = self.base.get_hdri_info();
        controls_request.current_fps = Some(self.base.fps_tracker.fps());
        let full_output = if self.base.key_handler.show_ui {
            self.base.render_ui(core, |ctx| {
                // transparent
                ctx.style_mut(|style| {
                    style.visuals.window_fill = egui::Color32::from_rgba_premultiplied(0, 0, 0, 180);
                });
                egui::Window::new("Shader Settings").show(ctx, |ui| {
                    egui::CollapsingHeader::new("Media").default_open(true).show(ui, |ui| {
                        ShaderControls::render_media_panel(
                            ui,
                            &mut controls_request,
                            using_video_texture,
                            video_info,
                            using_hdri_texture,
                            hdri_info
                        );
                    });
                    ui.separator();

                    egui::CollapsingHeader::new("Basic Parameters").default_open(true).show(ui, |ui| {
                        changed |= ui.add(egui::Slider::new(&mut params.branches, -20.0..=20.0).text("Branches")).changed();
                        changed |= ui.add(egui::Slider::new(&mut params.scale, 0.0..=2.0).text("Scale")).changed();
                        changed |= ui.add(egui::Slider::new(&mut params.zoom, 0.1..=5.0).text("Zoom")).changed();
                    });
                    
                    egui::CollapsingHeader::new("sty").default_open(false).show(ui, |ui| {
                        let mut use_anim = params.use_animation > 0.5;
                        if ui.checkbox(&mut use_anim, "Enable Animation").changed() {
                            changed = true;
                            params.use_animation = if use_anim { 1.0 } else { 0.0 };
                        }
                        if use_anim {
                            changed |= ui.add(egui::Slider::new(&mut params.time_scale, -5.0..=5.0).text("Animation Speed")).changed();
                        }
                        changed |= ui.add(egui::Slider::new(&mut params.rotation, -6.28..=6.28).text("Rotation")).changed();
                    });
    
                    egui::CollapsingHeader::new("anim").default_open(false).show(ui, |ui| {
                        changed |= ui.add(egui::Slider::new(&mut params.iterations, -10.0..=10.0).text("Iterations")).changed();
                        changed |= ui.add(egui::Slider::new(&mut params.smoothing, -1.0..=1.0).text("Smoothing")).changed();
                    });
                    
                    egui::CollapsingHeader::new("Tex Offset").default_open(false).show(ui, |ui| {
                        changed |= ui.add(egui::Slider::new(&mut params.offset_x, -1.0..=1.0).text("X")).changed();
                        changed |= ui.add(egui::Slider::new(&mut params.offset_y, -1.0..=1.0).text("Y")).changed();
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
        self.base.apply_control_request(controls_request.clone());
        self.base.handle_video_requests(core, &controls_request);
        self.base.handle_hdri_requests(core, &controls_request);
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
        let mut encoder = core.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Render Encoder"),
        });
        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Main Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            render_pass.set_pipeline(&self.base.renderer.render_pipeline);
            render_pass.set_vertex_buffer(0, self.base.renderer.vertex_buffer.slice(..));
            
            // Set the appropriate bind group at group 0 based on pipeline layout kind
            let layout_kind = VIDEO_SHADER_SEGMENTS[self.current_shader_index].layout;
            match layout_kind {
                PipelineLayoutKind::Texture => {
                    if self.base.using_video_texture {
                        if let Some(video_manager) = &self.base.video_texture_manager {
                            render_pass.set_bind_group(0, &video_manager.texture_manager().bind_group, &[]);
                        }
                    } else if let Some(texture_manager) = &self.base.texture_manager {
                        render_pass.set_bind_group(0, &texture_manager.bind_group, &[]);
                    }
                }
                PipelineLayoutKind::Buffer => {
                    // For Buffer layout, set a uniform buffer at group 0 (e.g., time_uniform)
                    render_pass.set_bind_group(0, &self.base.time_uniform.bind_group, &[]);
                }
            }
            // Time (group 1)
            render_pass.set_bind_group(1, &self.base.time_uniform.bind_group, &[]);
            // Params (group 2) - select correct bind group by params_kind
            let params_bind_group = match VIDEO_SHADER_SEGMENTS[self.current_shader_index].params_kind {
                ParamsKind::Payme => &self.params_uniform.bind_group,
                ParamsKind::GaborNoise => &self.gabornoise_params.bind_group,
                ParamsKind::Sinh => &self.sinh_params.bind_group,
                // Add more as needed for other ParamsKind
                _ => &self.params_uniform.bind_group, // fallback
            };
            render_pass.set_bind_group(2, params_bind_group, &[]);
            // Resolution (group 3)
            render_pass.set_bind_group(3, &self.base.resolution_uniform.bind_group, &[]);
            render_pass.draw(0..4, 0..1);
        }
        self.base.handle_render_output(core, &view, full_output, &mut encoder);
        core.queue.submit(Some(encoder.finish()));
        output.present();
        Ok(())
    }
    fn resize(&mut self, core: &Core) {
        self.base.update_resolution(&core.queue, core.size);
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