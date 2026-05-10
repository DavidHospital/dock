use std::{
    ffi::{CStr, c_void},
    sync::Arc,
};

use anyhow::{Context, anyhow};
use ash::{
    Entry, Instance,
    ext::debug_utils,
    khr::{surface, swapchain, wayland_surface},
    prelude::VkResult,
    vk::{self},
};
use wayland_client::{
    Proxy,
    protocol::{wl_display::WlDisplay, wl_surface::WlSurface},
};

struct RenderPipeline {
    device: Arc<ash::Device>,
    swap_chain: (
        swapchain::Device,
        vk::SwapchainKHR,
        Vec<vk::ImageView>,
        vk::Extent2D,
    ),
    render_pass: vk::RenderPass,
    vert_module: vk::ShaderModule,
    frag_module: vk::ShaderModule,
    descriptor_set_layouts: Vec<vk::DescriptorSetLayout>,
    pipeline_layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    framebuffers: Vec<vk::Framebuffer>,
    command_pool: vk::CommandPool,
    command_buffers: Vec<vk::CommandBuffer>,
    image_available_sem: vk::Semaphore,
    render_finished_sem: vk::Semaphore,
    in_flight_fence: vk::Fence,
    staging_buffer: vk::Buffer,
    staging_buffer_mem: vk::DeviceMemory,
    descriptor_sets: (vk::DescriptorPool, Vec<vk::DescriptorSet>),
}

impl Drop for RenderPipeline {
    fn drop(&mut self) {
        unsafe {
            let _ = self
                .device
                .free_descriptor_sets(self.descriptor_sets.0, &self.descriptor_sets.1);
            self.device
                .destroy_descriptor_pool(self.descriptor_sets.0, None);
            self.device.free_memory(self.staging_buffer_mem, None);
            self.device.destroy_buffer(self.staging_buffer, None);
            let _ = self.device.reset_fences(&[self.in_flight_fence]);
            self.device.destroy_fence(self.in_flight_fence, None);
            self.device
                .destroy_semaphore(self.render_finished_sem, None);
            self.device
                .destroy_semaphore(self.image_available_sem, None);
            self.device.destroy_command_pool(self.command_pool, None);
            for framebuffer in &self.framebuffers {
                self.device.destroy_framebuffer(*framebuffer, None);
            }
            self.device.destroy_pipeline(self.pipeline, None);
            self.device
                .destroy_pipeline_layout(self.pipeline_layout, None);
            for dsl in &self.descriptor_set_layouts {
                self.device.destroy_descriptor_set_layout(*dsl, None);
            }
            self.device.destroy_render_pass(self.render_pass, None);
            self.device.destroy_shader_module(self.vert_module, None);
            self.device.destroy_shader_module(self.frag_module, None);
            for image_view in &self.swap_chain.2 {
                self.device.destroy_image_view(*image_view, None);
            }
            self.swap_chain.0.destroy_swapchain(self.swap_chain.1, None);
        }
    }
}

#[allow(unused)]
pub struct Vk {
    entry: Entry,
    instance: Instance,
    debug_instance: debug_utils::Instance,
    debug_messenger: vk::DebugUtilsMessengerEXT,
    wl_instance: wayland_surface::Instance,
    wl_surface: vk::SurfaceKHR,
    physical_device: vk::PhysicalDevice,
    instance_khr: surface::Instance,
    device: Arc<ash::Device>,
    graphics_queue: vk::Queue,
    present_queue: Option<vk::Queue>,
    queue_families: QueueFamilies,
    device_details: SurfaceKHRDetails,
    render_pipeline: Option<RenderPipeline>,
}

impl Drop for Vk {
    fn drop(&mut self) {
        if let Some(render_pipeline) = self.render_pipeline.take() {
            drop(render_pipeline);
        }
        unsafe {
            self.device.destroy_device(None);
            self.instance_khr.destroy_surface(self.wl_surface, None);
            self.debug_instance
                .destroy_debug_utils_messenger(self.debug_messenger, None);
            self.instance.destroy_instance(None);
        }
    }
}

impl Vk {
    pub fn new(display: &WlDisplay, surface: &WlSurface) -> anyhow::Result<Self> {
        let entry = unsafe { Entry::load()? };

        let extensions = vec![
            ash::khr::surface::NAME.as_ptr(),
            ash::khr::wayland_surface::NAME.as_ptr(),
            ash::ext::debug_utils::NAME.as_ptr(),
        ];
        let layers = [c"VK_LAYER_KHRONOS_validation".as_ptr()];

        let app_info = vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_3);

        let instance_create_info = vk::InstanceCreateInfo::default()
            .application_info(&app_info)
            .enabled_layer_names(&layers[..])
            .enabled_extension_names(&extensions);

        unsafe {
            let instance = entry.create_instance(&instance_create_info, None)?;

            let debug_info = vk::DebugUtilsMessengerCreateInfoEXT::default()
                .message_severity(
                    vk::DebugUtilsMessageSeverityFlagsEXT::ERROR
                        | vk::DebugUtilsMessageSeverityFlagsEXT::WARNING,
                )
                .message_type(vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION)
                .pfn_user_callback(Some(vulkan_debug_callback));
            let debug_instance = debug_utils::Instance::new(&entry, &instance);
            let debug_messenger = debug_instance.create_debug_utils_messenger(&debug_info, None)?;

            let wl_instance = wayland_surface::Instance::new(&entry, &instance);
            let instance_khr = surface::Instance::new(&entry, &instance);

            let display = display.id().as_ptr() as *mut c_void;
            let surface = surface.id().as_ptr() as *mut c_void;

            let wl_surface_info = vk::WaylandSurfaceCreateInfoKHR {
                display,
                surface,
                ..Default::default()
            };
            let wl_surface = wl_instance.create_wayland_surface(&wl_surface_info, None)?;

            let (physical_device, queue_families) = instance
                .enumerate_physical_devices()?
                .into_iter()
                .find_map(|p_device| {
                    if !is_device_suitable(&instance, p_device) {
                        return None;
                    }

                    let families = QueueFamilies::new(
                        &instance,
                        &instance_khr,
                        &wl_instance,
                        display,
                        wl_surface,
                        p_device,
                    )
                    .ok()?;
                    Some((p_device, families))
                })
                .with_context(|| "Cound not find suitable physical device")?;

            let queue_family_indices = queue_families.incides();
            let queue_create_infos = queue_family_indices
                .iter()
                .map(|&i| {
                    vk::DeviceQueueCreateInfo::default()
                        .queue_family_index(i)
                        .queue_priorities(&[1.0])
                })
                .collect::<Vec<_>>();

            let device_extensions = [ash::khr::swapchain::NAME.as_ptr()];
            let device_create_info = vk::DeviceCreateInfo::default()
                .enabled_extension_names(&device_extensions)
                .queue_create_infos(&queue_create_infos);

            let device = instance.create_device(physical_device, &device_create_info, None)?;
            let graphics_queue = device.get_device_queue(queue_families.graphics_family, 0);
            let present_queue = queue_families
                .present_family
                .map(|present_family| device.get_device_queue(present_family, 0));
            let device_details =
                SurfaceKHRDetails::new(&instance, &instance_khr, physical_device, wl_surface)?;

            let device = Arc::new(device);

            Ok(Self {
                entry,
                instance,
                debug_instance,
                debug_messenger,
                wl_instance,
                wl_surface,
                physical_device,
                instance_khr,
                device,
                graphics_queue,
                present_queue,
                queue_families,
                device_details,
                render_pipeline: None,
            })
        }
    }

    pub fn init_pipeline(&mut self, width: u32, height: u32) -> anyhow::Result<()> {
        if let Some(render_pipeline) = self.render_pipeline.take() {
            unsafe {
                self.device.device_wait_idle()?;
            }

            drop(render_pipeline);
        }

        let surface_format = self.device_details.choose_surface_format()?;
        let extent = self.device_details.choose_extent(width, height);
        let queue_family_indices = self.queue_families.incides();

        let sc_create_info = vk::SwapchainCreateInfoKHR::default()
            .surface(self.wl_surface)
            .min_image_count(self.device_details.surface_caps.min_image_count)
            .image_format(surface_format.format)
            .image_color_space(surface_format.color_space)
            .image_extent(extent)
            .image_array_layers(1)
            .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT)
            .image_sharing_mode(self.queue_families.sharing_mode())
            .queue_family_indices(&queue_family_indices)
            .pre_transform(self.device_details.surface_caps.current_transform)
            .composite_alpha(self.device_details.choose_composite_alpha_flag())
            .present_mode(self.device_details.choose_present_mode())
            .clipped(true)
            .old_swapchain(vk::SwapchainKHR::null());

        let sc_device = swapchain::Device::new(&self.instance, &self.device);
        let swap_chain = unsafe { sc_device.create_swapchain(&sc_create_info, None)? };
        let sc_images = unsafe { sc_device.get_swapchain_images(swap_chain)? };

        let image_view_create_infos = sc_images
            .iter()
            .map(|&image| {
                vk::ImageViewCreateInfo::default()
                    .image(image)
                    .format(surface_format.format)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .components(vk::ComponentMapping::default())
                    .subresource_range(
                        vk::ImageSubresourceRange::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .base_mip_level(0)
                            .level_count(1)
                            .base_array_layer(0)
                            .layer_count(1),
                    )
            })
            .collect::<Vec<_>>();

        let mut image_views = Vec::with_capacity(image_view_create_infos.len());
        for ci in image_view_create_infos {
            image_views.push(unsafe { self.device.create_image_view(&ci, None)? });
        }

        let vert_code = include_bytes!("shader.vert.spv");
        let vert_code = bytes_to_spv(vert_code);
        let vert_create_info = vk::ShaderModuleCreateInfo::default().code(&vert_code);
        let vert_module = unsafe { self.device.create_shader_module(&vert_create_info, None)? };

        let frag_code = include_bytes!("shader.frag.spv");
        let frag_code = bytes_to_spv(frag_code);
        let frag_create_info = vk::ShaderModuleCreateInfo::default().code(&frag_code);
        let frag_module = unsafe { self.device.create_shader_module(&frag_create_info, None)? };

        let color_attachments = [vk::AttachmentDescription::default()
            .format(self.device_details.choose_surface_format()?.format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::PRESENT_SRC_KHR)];

        let color_attachment_refs = [vk::AttachmentReference::default()
            .attachment(0)
            .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];

        let subpasses = [vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(&color_attachment_refs)];

        let render_pass_create_info = vk::RenderPassCreateInfo::default()
            .attachments(&color_attachments)
            .subpasses(&subpasses);

        let render_pass = unsafe {
            self.device
                .create_render_pass(&render_pass_create_info, None)?
        };

        let stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .name(&c"vs_main")
                .module(vert_module),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .name(&c"fs_main")
                .module(frag_module),
        ];
        let vert_input_state = vk::PipelineVertexInputStateCreateInfo::default();
        let input_assembly_state = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewports = [vk::Viewport {
            x: 0.,
            y: 0.,
            width: width as f32,
            height: height as f32,
            min_depth: 0.,
            max_depth: 1.,
        }];
        let scissors = [vk::Rect2D::default()
            .offset(vk::Offset2D { x: 0, y: 0 })
            .extent(extent)];
        let viewport_state = vk::PipelineViewportStateCreateInfo::default()
            .viewports(&viewports)
            .scissors(&scissors);
        let rasterization_state = vk::PipelineRasterizationStateCreateInfo::default()
            .cull_mode(vk::CullModeFlags::BACK)
            .polygon_mode(vk::PolygonMode::FILL)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            .line_width(1.0);
        let multisample_state = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let color_blend_attachments = [vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA)
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_COLOR)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ZERO)
            .alpha_blend_op(vk::BlendOp::ADD)];
        let color_blend_state = vk::PipelineColorBlendStateCreateInfo::default()
            .logic_op_enable(false)
            .attachments(&color_blend_attachments);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic_state =
            vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);

        let dsl_bindings = [vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)];
        let descriptor_set_layouts = unsafe {
            vec![self.device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&dsl_bindings),
                None,
            )?]
        };
        let pipeline_layout_create_info =
            vk::PipelineLayoutCreateInfo::default().set_layouts(&descriptor_set_layouts);
        let pipeline_layout = unsafe {
            self.device
                .create_pipeline_layout(&pipeline_layout_create_info, None)?
        };
        let pipeline_create_infos = [vk::GraphicsPipelineCreateInfo::default()
            .layout(pipeline_layout)
            .stages(&stages)
            .vertex_input_state(&vert_input_state)
            .input_assembly_state(&input_assembly_state)
            .viewport_state(&viewport_state)
            .multisample_state(&multisample_state)
            .rasterization_state(&rasterization_state)
            .color_blend_state(&color_blend_state)
            .dynamic_state(&dynamic_state)
            .render_pass(render_pass)
            .subpass(0)];
        let pipeline = unsafe {
            self.device
                .create_graphics_pipelines(vk::PipelineCache::null(), &pipeline_create_infos, None)
                .map_err(|e| e.1)?[0]
        };

        let mut framebuffers = Vec::with_capacity(image_views.len());
        for image_view in &image_views {
            let attachments = [*image_view];
            let frame_buffer_create_info = vk::FramebufferCreateInfo::default()
                .render_pass(render_pass)
                .attachments(&attachments)
                .width(extent.width)
                .height(extent.height)
                .layers(1);

            framebuffers.push(unsafe {
                self.device
                    .create_framebuffer(&frame_buffer_create_info, None)?
            });
        }

        let command_pool_create_info = vk::CommandPoolCreateInfo::default()
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER)
            .queue_family_index(self.queue_families.graphics_family);

        let command_pool = unsafe {
            self.device
                .create_command_pool(&command_pool_create_info, None)?
        };

        let command_buffer_create_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);

        let command_buffers = unsafe {
            self.device
                .allocate_command_buffers(&command_buffer_create_info)?
        };

        let image_available_sem = unsafe {
            self.device
                .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)?
        };
        let render_finished_sem = unsafe {
            self.device
                .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)?
        };
        let in_flight_fence = unsafe {
            self.device.create_fence(
                &vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED),
                None,
            )?
        };

        let buf_size = extent.width * extent.height / 8;
        let staging_buffer_create_info = vk::BufferCreateInfo::default()
            .size(buf_size as u64)
            .usage(vk::BufferUsageFlags::STORAGE_BUFFER);

        let staging_buffer = unsafe {
            self.device
                .create_buffer(&staging_buffer_create_info, None)?
        };

        let mem_reqs = unsafe { self.device.get_buffer_memory_requirements(staging_buffer) };
        let memory_type_index = self.device_details.find_memory_index(
            mem_reqs.memory_type_bits,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::DEVICE_LOCAL,
        );
        let staging_buffer_mem = unsafe {
            self.device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(mem_reqs.size) // use actual required size too
                    .memory_type_index(memory_type_index),
                None,
            )?
        };
        unsafe {
            self.device
                .bind_buffer_memory(staging_buffer, staging_buffer_mem, 0)?
        };

        let pool_sizes = [vk::DescriptorPoolSize {
            ty: vk::DescriptorType::STORAGE_BUFFER,
            descriptor_count: 1,
        }];
        let descriptor_pool = unsafe {
            self.device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .flags(vk::DescriptorPoolCreateFlags::FREE_DESCRIPTOR_SET)
                    .pool_sizes(&pool_sizes)
                    .max_sets(1),
                None,
            )?
        };
        let descriptor_sets = unsafe {
            self.device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(descriptor_pool)
                    .set_layouts(&descriptor_set_layouts),
            )?
        };

        let descriptor_buffer_info = [vk::DescriptorBufferInfo::default()
            .buffer(staging_buffer)
            .offset(0)
            .range(vk::WHOLE_SIZE)];

        let descriptor_writes = [vk::WriteDescriptorSet::default()
            .dst_set(descriptor_sets[0])
            .dst_binding(0)
            .dst_array_element(0)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(&descriptor_buffer_info)];

        unsafe { self.device.update_descriptor_sets(&descriptor_writes, &[]) };

        self.render_pipeline = Some(RenderPipeline {
            device: self.device.clone(),
            swap_chain: (sc_device, swap_chain, image_views, extent),
            vert_module,
            frag_module,
            descriptor_set_layouts,
            pipeline_layout,
            pipeline,
            framebuffers,
            command_pool,
            command_buffers,
            render_pass,
            image_available_sem,
            render_finished_sem,
            in_flight_fence,
            staging_buffer,
            staging_buffer_mem,
            descriptor_sets: (descriptor_pool, descriptor_sets),
        });

        Ok(())
    }

    pub fn draw_frame(&self, frame_data: &[u8]) -> anyhow::Result<()> {
        let Some(rp) = self.render_pipeline.as_ref() else {
            anyhow::bail!("Render pipeline not configured");
        };

        let (sc_device, swap_chain, _, _) = &rp.swap_chain;

        unsafe {
            self.device
                .wait_for_fences(&[rp.in_flight_fence], true, u64::MAX)?;
            self.device.reset_fences(&[rp.in_flight_fence])?;

            let (image_index, _) = sc_device.acquire_next_image(
                *swap_chain,
                u64::MAX,
                rp.image_available_sem,
                vk::Fence::null(),
            )?;

            self.device.reset_command_buffer(
                rp.command_buffers[0],
                vk::CommandBufferResetFlags::empty(),
            )?;
            self.record_command_buffer(
                0,
                image_index as usize,
                frame_data.as_ptr() as *const c_void,
            )?;

            let wait_semaphores = [rp.image_available_sem];
            let signal_semaphores = [rp.render_finished_sem];
            let wait_stages = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
            let submit_info = vk::SubmitInfo::default()
                .wait_semaphores(&wait_semaphores)
                .wait_dst_stage_mask(&wait_stages)
                .command_buffers(&rp.command_buffers[..1])
                .signal_semaphores(&signal_semaphores);
            self.device
                .queue_submit(self.graphics_queue, &[submit_info], rp.in_flight_fence)?;

            let swapchains = [*swap_chain];
            let image_indices = [image_index];
            let present_info = vk::PresentInfoKHR::default()
                .wait_semaphores(&signal_semaphores)
                .swapchains(&swapchains)
                .image_indices(&image_indices);
            sc_device.queue_present(
                self.present_queue.unwrap_or(self.graphics_queue),
                &present_info,
            )?;

            self.device
                .queue_wait_idle(self.present_queue.unwrap_or(self.graphics_queue))?;
        }

        Ok(())
    }

    fn record_command_buffer(
        &self,
        command_buffer_index: usize,
        image_index: usize,
        frame_data: *const c_void,
    ) -> VkResult<()> {
        let Some(render_pipeline) = &self.render_pipeline else {
            return Ok(());
        };

        let command_buffer = render_pipeline.command_buffers[command_buffer_index];
        let extent = render_pipeline.swap_chain.3;
        let buf_size = extent.width * extent.height / 8;

        let begin_info = vk::CommandBufferBeginInfo::default();
        unsafe {
            self.device
                .begin_command_buffer(command_buffer, &begin_info)?
        };

        let clear_values = [vk::ClearValue {
            color: vk::ClearColorValue {
                float32: [0.0, 0.0, 0.0, 1.0],
            },
        }];
        let render_pass_info = vk::RenderPassBeginInfo::default()
            .render_pass(render_pipeline.render_pass)
            .framebuffer(render_pipeline.framebuffers[image_index])
            .render_area(
                vk::Rect2D::default()
                    .offset(vk::Offset2D { x: 0, y: 0 })
                    .extent(extent),
            )
            .clear_values(&clear_values);

        unsafe {
            let buf = self.device.map_memory(
                render_pipeline.staging_buffer_mem,
                0,
                buf_size as u64,
                vk::MemoryMapFlags::empty(),
            )?;
            std::ptr::copy_nonoverlapping(frame_data, buf, buf_size as usize);
            self.device.unmap_memory(render_pipeline.staging_buffer_mem);

            self.device.cmd_begin_render_pass(
                command_buffer,
                &render_pass_info,
                vk::SubpassContents::INLINE,
            );
            self.device.cmd_bind_pipeline(
                command_buffer,
                vk::PipelineBindPoint::GRAPHICS,
                render_pipeline.pipeline,
            );
            let viewports = [vk::Viewport {
                x: 0.0,
                y: 0.0,
                width: render_pipeline.swap_chain.3.width as f32,
                height: render_pipeline.swap_chain.3.height as f32,
                min_depth: 0.0,
                max_depth: 1.0,
            }];
            self.device.cmd_set_viewport(command_buffer, 0, &viewports);
            let scissors = [vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: render_pipeline.swap_chain.3,
            }];
            self.device.cmd_set_scissor(command_buffer, 0, &scissors);
            self.device.cmd_bind_descriptor_sets(
                command_buffer,
                vk::PipelineBindPoint::GRAPHICS,
                render_pipeline.pipeline_layout,
                0,
                &render_pipeline.descriptor_sets.1,
                &[],
            );
            self.device.cmd_draw(command_buffer, 3, 1, 0, 0);
            self.device.cmd_end_render_pass(command_buffer);
            self.device.end_command_buffer(command_buffer)?;
        }

        Ok(())
    }
}

extern "system" fn vulkan_debug_callback(
    severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    message_type: vk::DebugUtilsMessageTypeFlagsEXT,
    callback_data: *const vk::DebugUtilsMessengerCallbackDataEXT,
    _user_data: *mut std::ffi::c_void,
) -> vk::Bool32 {
    let message = unsafe { CStr::from_ptr((*callback_data).p_message) };

    eprintln!("[VULKAN]: {:?} {:?} {:?}", severity, message_type, message,);

    vk::FALSE
}

fn is_device_suitable(instance: &Instance, device: vk::PhysicalDevice) -> bool {
    let Ok(extensions) = (unsafe { instance.enumerate_device_extension_properties(device) }) else {
        return false;
    };

    let has_swapchain = extensions
        .iter()
        .any(|e| e.extension_name_as_c_str().ok() == Some(ash::khr::swapchain::NAME));
    if !has_swapchain {
        return false;
    }

    true
}

struct QueueFamilies {
    graphics_family: u32,
    present_family: Option<u32>,
}

impl QueueFamilies {
    fn new(
        instance: &Instance,
        instance_khr: &surface::Instance,
        wl_instance: &wayland_surface::Instance,
        wl_display: *mut vk::wl_display,
        surface_khr: vk::SurfaceKHR,
        device: vk::PhysicalDevice,
    ) -> anyhow::Result<Self> {
        let queue_families =
            unsafe { instance.get_physical_device_queue_family_properties(device) };

        let mut present_family = None;

        let graphics_family = queue_families
            .iter()
            .enumerate()
            .find_map(|(i, props)| {
                if present_family.is_none()
                    && unsafe {
                        instance_khr
                            .get_physical_device_surface_support(device, i as u32, surface_khr)
                            .ok()?
                    }
                {
                    present_family = Some(i as u32);
                }

                let supports_graphics = props.queue_flags.contains(vk::QueueFlags::GRAPHICS);
                let supports_present = unsafe {
                    wl_instance.get_physical_device_wayland_presentation_support(
                        device,
                        i as u32,
                        &mut *wl_display,
                    )
                };
                (supports_graphics && supports_present).then_some(i as u32)
            })
            .context("No queue family with graphics support")?;

        Ok(Self {
            graphics_family,
            present_family,
        })
    }

    fn incides(&self) -> Vec<u32> {
        let mut indices = Vec::with_capacity(2);
        indices.push(self.graphics_family);

        if let Some(present_family) = self.present_family
            && present_family != self.graphics_family
        {
            indices.push(present_family);
        }

        indices
    }

    fn sharing_mode(&self) -> vk::SharingMode {
        if self
            .present_family
            .is_some_and(|present_family| present_family == self.graphics_family)
        {
            vk::SharingMode::EXCLUSIVE
        } else {
            vk::SharingMode::CONCURRENT
        }
    }
}

#[allow(unused)]
struct SurfaceKHRDetails {
    surface_caps: vk::SurfaceCapabilitiesKHR,
    surface_formats: Vec<vk::SurfaceFormatKHR>,
    present_modes: Vec<vk::PresentModeKHR>,
    mem_props: vk::PhysicalDeviceMemoryProperties,
}

impl SurfaceKHRDetails {
    fn new(
        instance: &Instance,
        instance_khr: &surface::Instance,
        device: vk::PhysicalDevice,
        surface: vk::SurfaceKHR,
    ) -> VkResult<Self> {
        unsafe {
            let surface_caps =
                instance_khr.get_physical_device_surface_capabilities(device, surface)?;
            let surface_formats =
                instance_khr.get_physical_device_surface_formats(device, surface)?;
            let present_modes =
                instance_khr.get_physical_device_surface_present_modes(device, surface)?;

            let mem_props = instance.get_physical_device_memory_properties(device);

            Ok(Self {
                surface_caps,
                surface_formats,
                present_modes,
                mem_props,
            })
        }
    }

    fn choose_surface_format(&self) -> anyhow::Result<vk::SurfaceFormatKHR> {
        for s_format in &self.surface_formats {
            if s_format.format == vk::Format::B8G8R8A8_SRGB
                && s_format.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR
            {
                return Ok(*s_format);
            }
        }

        Err(anyhow!(
            "No available surface format from formats: [{:?}]",
            self.surface_formats
        ))
    }

    fn choose_present_mode(&self) -> vk::PresentModeKHR {
        return vk::PresentModeKHR::FIFO;
    }

    fn choose_extent(&self, width: u32, height: u32) -> vk::Extent2D {
        vk::Extent2D { width, height }
    }

    fn choose_composite_alpha_flag(&self) -> vk::CompositeAlphaFlagsKHR {
        let mut flag = vk::CompositeAlphaFlagsKHR::empty();
        flag |= self.surface_caps.supported_composite_alpha
            & vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED;
        if !flag.is_empty() {
            return flag;
        }
        flag |= self.surface_caps.supported_composite_alpha & vk::CompositeAlphaFlagsKHR::INHERIT;
        if !flag.is_empty() {
            return flag;
        }
        return vk::CompositeAlphaFlagsKHR::OPAQUE;
    }

    fn find_memory_index(&self, type_filter: u32, flags: vk::MemoryPropertyFlags) -> u32 {
        for i in 0..self.mem_props.memory_type_count {
            let type_matches = type_filter & (1 << i) != 0;
            let flags_match = self.mem_props.memory_types[i as usize]
                .property_flags
                .contains(flags);
            if type_matches && flags_match {
                return i;
            }
        }
        panic!("no suitable memory type found");
    }
}

fn bytes_to_spv(bytes: &[u8]) -> Vec<u32> {
    assert!(bytes.len() % 4 == 0);
    bytes
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
        .collect()
}
