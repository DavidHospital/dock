mod render;

use wayland_client::Dispatch;
use wayland_client::protocol::wl_callback;
use wayland_client::{
    Connection, QueueHandle, delegate_noop,
    protocol::{
        wl_buffer::WlBuffer,
        wl_compositor::WlCompositor,
        wl_display::WlDisplay,
        wl_registry,
        wl_shm::{self, WlShm},
        wl_shm_pool::WlShmPool,
        wl_surface::WlSurface,
    },
};
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

        Self {
            globals,
            surface,
            ctx,
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
                state.ctx.draw_frame().unwrap();

                state.surface.frame(qhandle, ());
                println!("Configured");
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for State {
    fn event(
        state: &mut Self,
        _proxy: &wl_callback::WlCallback,
        event: <wl_callback::WlCallback as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        qhandle: &QueueHandle<Self>,
    ) {
        println!("Frame");
        if let wl_callback::Event::Done { callback_data: _ } = event {
            state.ctx.draw_frame().unwrap();

            state.surface.frame(qhandle, ());
            state.surface.commit();
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

    while event_queue.blocking_dispatch(&mut state).is_ok() {}

    Ok(())
}
