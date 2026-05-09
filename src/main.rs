mod render;

use std::os::fd::{AsRawFd, BorrowedFd};

use memmap2::MmapMut;
use wayland_client::{
    Connection, QueueHandle, delegate_noop, protocol::{
        wl_buffer::WlBuffer, wl_compositor::WlCompositor, wl_display::WlDisplay, wl_registry, wl_shm::{self, WlShm}, wl_shm_pool::WlShmPool, wl_surface::WlSurface
    }
};
use wayland_client::Dispatch;
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{Layer, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{self, Anchor, ZwlrLayerSurfaceV1},
};

use crate::render::Vk;

struct State {
    globals: Globals,
    surface: WlSurface,
    ctx: Vk,
}

impl State {
    fn new(display: &WlDisplay, globals: Globals, qh: &QueueHandle<Self>) -> Self {
        let surface = globals.compositor.create_surface(qh, ());
        let layer_surface = globals.layer_shell.get_layer_surface(
            &surface,
            None,
            Layer::Bottom,
            "dock".to_string(),
            qh,
            (),
        );
        layer_surface.set_exclusive_zone(48);
        layer_surface.set_anchor(Anchor::Top | Anchor::Left | Anchor::Right);
        layer_surface.set_size(0, 48);
        surface.commit();

        let ctx = Vk::new(display, &surface).unwrap();
        // let pipeline = DockPipeline::new(&ctx).unwrap();

        Self {
            globals,
            surface,
            ctx,
            // pipeline,
        }
    }
}

struct Globals {
    compositor: WlCompositor,
    shm: WlShm,
    layer_shell: ZwlrLayerShellV1,
}

struct Bootstrap {
    compositor: Option<WlCompositor>,
    shm: Option<WlShm>,
    layer_shell: Option<ZwlrLayerShellV1>,
}

impl TryFrom<Bootstrap> for Globals {
    type Error = Bootstrap;

    fn try_from(bootstrap: Bootstrap) -> Result<Self, Self::Error> {
        match (bootstrap.compositor, bootstrap.shm, bootstrap.layer_shell) {
            (Some(compositor), Some(shm), Some(layer_shell)) => Ok(Globals {
                compositor,
                shm,
                layer_shell,
            }),
            (compositor, shm, layer_shell) => Err(Bootstrap {
                compositor,
                shm,
                layer_shell,
            }),
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for Bootstrap {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: <wl_registry::WlRegistry as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        qhandle: &wayland_client::QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match &interface[..] {
                "wl_compositor" => {
                    let compositor =
                        registry.bind::<WlCompositor, _, _>(name, version, qhandle, ());
                    state.compositor = Some(compositor);
                }
                "wl_shm" => {
                    let shm = registry.bind::<WlShm, _, _>(name, version, qhandle, ());
                    state.shm = Some(shm);
                }
                "zwlr_layer_shell_v1" => {
                    let layer_shell =
                        registry.bind::<ZwlrLayerShellV1, _, _>(name, version, qhandle, ());
                    state.layer_shell = Some(layer_shell);
                }
                _ => {}
            }
            // println!("[{name}] {interface} [v{version}]");
        }
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, ()> for State {
    fn event(
        state: &mut Self,
        proxy: &ZwlrLayerSurfaceV1,
        event: <ZwlrLayerSurfaceV1 as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        qhandle: &wayland_client::QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                proxy.ack_configure(serial);

                state.ctx.init_pipeline(width, height).unwrap();

                let buffers = create_buffers(&state.globals.shm, width, height, qhandle).unwrap();
                state.surface.attach(Some(&buffers[0]), 0, 0);
                state.surface.damage(0, 0, i32::MAX, i32::MAX);
                state.surface.commit();
            }
            _ => {}
        }
    }
}

delegate_noop!(Bootstrap: ignore WlCompositor);
delegate_noop!(Bootstrap: ignore WlShm);
delegate_noop!(Bootstrap: ignore ZwlrLayerShellV1);

delegate_noop!(State: ignore WlSurface);
delegate_noop!(State: ignore WlShmPool);
delegate_noop!(State: ignore WlBuffer);

fn main() -> anyhow::Result<()> {
    let connection = Connection::connect_to_env()?;

    let display = connection.display();

    let mut bs_event_queue = connection.new_event_queue();
    let bootstrap_qh = bs_event_queue.handle();

    let _registry = display.get_registry(&bootstrap_qh, ());

    let mut bootstrap = Bootstrap {
        compositor: None,
        shm: None,
        layer_shell: None,
    };

    let globals = loop {
        bs_event_queue.blocking_dispatch(&mut bootstrap)?;
        match Globals::try_from(bootstrap) {
            Ok(globals) => break globals,
            Err(bs) => bootstrap = bs,
        }
    };

    drop(bs_event_queue);
    drop(bootstrap_qh);

    let mut event_queue = connection.new_event_queue::<State>();
    let qh = event_queue.handle();

    let mut state = State::new(&display, globals, &qh);

    println!("Configured");

    while event_queue.blocking_dispatch(&mut state).is_ok() {}

    Ok(())
}

fn create_buffers<S: Dispatch<WlShmPool, ()> + Dispatch<WlBuffer, ()> + 'static>(
    shm: &WlShm,
    width: u32,
    height: u32,
    qh: &QueueHandle<S>,
) -> anyhow::Result<[WlBuffer; 2]> {
    let stride = width * 4;
    let size = stride * height;

    let owned_fd = rustix::fs::memfd_create("dock", rustix::fs::MemfdFlags::empty())?;
    rustix::fs::ftruncate(&owned_fd, size as u64)?;

    let pool = unsafe {
        let mut data = MmapMut::map_mut(&owned_fd)?;
        for word in 0..data.len() / 4 {
            // 0x192035
            data[word * 4 + 0] = 0x35;
            data[word * 4 + 1] = 0x20;
            data[word * 4 + 2] = 0x19;
            data[word * 4 + 3] = 165;
        }
        
        shm.create_pool(
            BorrowedFd::borrow_raw(owned_fd.as_raw_fd()),
            size as i32 * 2,
            &qh,
            (),
        )
    };

    let b1 = pool.create_buffer(
        0,
        width as i32,
        height as i32,
        stride as i32,
        wl_shm::Format::Argb8888,
        qh,
        (),
    );

    let b2 = pool.create_buffer(
        size as i32,
        width as i32,
        height as i32,
        stride as i32,
        wl_shm::Format::Argb8888,
        qh,
        (),
    );

    pool.destroy();

    Ok([b1, b2])
}
