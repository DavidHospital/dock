use std::ffi::{CStr, c_void};

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

pub struct Vk {
    entry: Entry,
    instance: Instance,
    debug_instance: debug_utils::Instance,
    debug_messenger: vk::DebugUtilsMessengerEXT,
    wl_instance: wayland_surface::Instance,
    wl_surface: vk::SurfaceKHR,
    physical_device: vk::PhysicalDevice,
    instance_khr: surface::Instance,
    device: ash::Device,
    graphics_queue: vk::Queue,
    present_queue: Option<vk::Queue>,
    queue_families: QueueFamilies,
    device_details: SurfaceKHRDetails,
    swap_chain: Option<(swapchain::Device, vk::SwapchainKHR)>,
    render_pass: Option<vk::RenderPass>,
    vertex_module: Option<vk::ShaderModule>,
    frag_module: Option<vk::ShaderModule>,
    pipeline_layout: Option<vk::PipelineLayout>,
    pipeline: Option<vk::Pipeline>,
}

impl Drop for Vk {
    fn drop(&mut self) {
        unsafe {
            if let Some(pipeline) = self.pipeline.take() {
                self.device.destroy_pipeline(pipeline, None);
            }
            if let Some(pipeline_layout) = self.pipeline_layout.take() {
                self.device.destroy_pipeline_layout(pipeline_layout, None);
            }
            if let Some(render_pass) = self.render_pass.take() {
                self.device.destroy_render_pass(render_pass, None);
            }
            if let Some(module) = self.vertex_module.take() {
                self.device.destroy_shader_module(module, None);
            }
            if let Some(module) = self.frag_module.take() {
                self.device.destroy_shader_module(module, None);
            }
            if let Some((device, swap_chain)) = self.swap_chain.take() {
                device.destroy_swapchain(swap_chain, None);
            }
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
                SurfaceKHRDetails::new(&instance_khr, physical_device, wl_surface)?;

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
                swap_chain: None,
                render_pass: None,
                pipeline_layout: None,
                pipeline: None,
                vertex_module: None,
                frag_module: None,
            })
        }
    }

    fn init_swap_chain(&mut self, width: u32, height: u32) -> anyhow::Result<()> {
        let surface_format = self.device_details.choose_surface_format()?;
        let queue_family_indices = self.queue_families.incides();

        let sc_create_info = vk::SwapchainCreateInfoKHR::default()
            .surface(self.wl_surface)
            .min_image_count(self.device_details.surface_caps.min_image_count)
            .image_format(surface_format.format)
            .image_color_space(surface_format.color_space)
            .image_extent(self.device_details.choose_extent(width, height))
            .image_array_layers(1)
            .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT)
            .image_sharing_mode(self.queue_families.sharing_mode())
            .queue_family_indices(&queue_family_indices)
            .pre_transform(self.device_details.surface_caps.current_transform)
            .composite_alpha(vk::CompositeAlphaFlagsKHR::OPAQUE)
            .present_mode(self.device_details.choose_present_mode())
            .clipped(true)
            .old_swapchain(vk::SwapchainKHR::null());

        if let Some((device, swap_chain)) = self.swap_chain.take() {
            unsafe {
                device.destroy_swapchain(swap_chain, None);
            }
        }

        let sc_device = swapchain::Device::new(&self.instance, &self.device);
        let swap_chain = unsafe { sc_device.create_swapchain(&sc_create_info, None)? };

        self.swap_chain = Some((sc_device, swap_chain));

        Ok(())
    }

    pub fn init_pipeline(&mut self, width: u32, height: u32) -> anyhow::Result<()> {
        self.init_swap_chain(width, height)?;

        if let Some(pipeline) = self.pipeline.take() {
            unsafe { self.device.destroy_pipeline(pipeline, None) };
        }

        if let Some(pipeline_layout) = self.pipeline_layout.take() {
            unsafe { self.device.destroy_pipeline_layout(pipeline_layout, None) };
        }

        if let Some(render_pass) = self.render_pass.take() {
            unsafe {
                self.device.destroy_render_pass(render_pass, None);
            }
        }

        if let Some(module) = self.vertex_module.take() {
            unsafe { self.device.destroy_shader_module(module, None) };
        }

        if let Some(module) = self.frag_module.take() {
            unsafe { self.device.destroy_shader_module(module, None) };
        }

        let shader_module = ShaderModule::new(include_str!("shader.wgsl"))?;

        let vertex_module = shader_module.vertex(&self.device)?;
        let frag_module = shader_module.fragment(&self.device)?;

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
                .module(vertex_module),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .name(&c"fs_main")
                .module(frag_module),
        ];
        let vertex_input_state = vk::PipelineVertexInputStateCreateInfo::default();
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
            .extent(self.device_details.choose_extent(width, height))];
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
            .blend_enable(false)];
        let color_blend_state = vk::PipelineColorBlendStateCreateInfo::default()
            .logic_op_enable(false)
            .attachments(&color_blend_attachments);
        let pipeline_layout_create_info = vk::PipelineLayoutCreateInfo::default();
        let pipeline_layout = unsafe {
            self.device
                .create_pipeline_layout(&pipeline_layout_create_info, None)?
        };
        let pipeline_create_infos = [vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input_state)
            .input_assembly_state(&input_assembly_state)
            .viewport_state(&viewport_state)
            .multisample_state(&multisample_state)
            .rasterization_state(&rasterization_state)
            .color_blend_state(&color_blend_state)
            .render_pass(render_pass)
            .subpass(0)];
        let pipeline = unsafe {
            self.device
                .create_graphics_pipelines(vk::PipelineCache::null(), &pipeline_create_infos, None)
                .map_err(|e| e.1)?[0]
        };

        self.pipeline = Some(pipeline);
        self.pipeline_layout = Some(pipeline_layout);
        self.vertex_module = Some(vertex_module);
        self.frag_module = Some(frag_module);

        Ok(())
    }
}

struct ShaderModule {
    module: naga::Module,
    module_info: naga::valid::ModuleInfo,
}

impl ShaderModule {
    fn new(wgsl_code: &str) -> anyhow::Result<Self> {
        let module = naga::front::wgsl::parse_str(wgsl_code)?;
        let module_info = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .subgroup_stages(naga::valid::ShaderStages::all())
        .subgroup_operations(naga::valid::SubgroupOperationSet::all())
        .validate(&module)?;

        Ok(Self {
            module,
            module_info,
        })
    }

    fn vertex(&self, device: &ash::Device) -> anyhow::Result<vk::ShaderModule> {
        let code = naga::back::spv::write_vec(
            &self.module,
            &self.module_info,
            &Default::default(),
            Some(&naga::back::spv::PipelineOptions {
                shader_stage: naga::ShaderStage::Vertex,
                entry_point: "vs_main".to_string(),
            }),
        )?;
        let create_info = vk::ShaderModuleCreateInfo::default().code(&code);

        let shader_module = unsafe { device.create_shader_module(&create_info, None)? };

        Ok(shader_module)
    }

    fn fragment(&self, device: &ash::Device) -> anyhow::Result<vk::ShaderModule> {
        let code = naga::back::spv::write_vec(
            &self.module,
            &self.module_info,
            &Default::default(),
            Some(&naga::back::spv::PipelineOptions {
                shader_stage: naga::ShaderStage::Fragment,
                entry_point: "fs_main".to_string(),
            }),
        )?;
        let create_info = vk::ShaderModuleCreateInfo::default().code(&code);

        let shader_module = unsafe { device.create_shader_module(&create_info, None)? };

        Ok(shader_module)
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

struct SurfaceKHRDetails {
    surface_caps: vk::SurfaceCapabilitiesKHR,
    surface_formats: Vec<vk::SurfaceFormatKHR>,
    present_modes: Vec<vk::PresentModeKHR>,
}

impl SurfaceKHRDetails {
    fn new(
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

            Ok(Self {
                surface_caps,
                surface_formats,
                present_modes,
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
}
