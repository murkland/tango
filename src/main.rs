#[macro_use]
extern crate lazy_static;

mod mgba;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    mgba::log::set_default_logger(Box::new(&|category, level, message| {
        log::info!("{}", message)
    }));

    let mut core = mgba::core::Core::new_gba("tango").unwrap();
    let rom_vf = mgba::vfile::VFile::open("bn6f.gba", 0).unwrap();
    core.load_rom(rom_vf);

    let (width, height) = core.desired_video_dimensions();
    let mut vbuf = vec![0u8; (width * height * 4) as usize];
    core.set_video_buffer(&mut vbuf, width.into());

    let event_loop = winit::event_loop::EventLoop::new();
    let mut input = winit_input_helper::WinitInputHelper::new();

    let window = winit::window::WindowBuilder::new()
        .with_title("tango")
        .with_inner_size(winit::dpi::LogicalSize::new(width, height))
        .build(&event_loop)
        .unwrap();

    let pixels = std::sync::Arc::new(std::sync::Mutex::new({
        let window_size = window.inner_size();
        let surface_texture =
            pixels::SurfaceTexture::new(window_size.width, window_size.height, &window);
        pixels::Pixels::new(width, height, surface_texture)?
    }));

    let mut thread = mgba::thread::Thread::new(&mut core);
    {
        let pixels = std::sync::Arc::clone(&pixels);
        thread.frame_callback = Some(Box::new(move || {
            pixels.lock().unwrap().get_frame().copy_from_slice(&vbuf);
        }));
    }
    thread.start();

    event_loop.run(move |event, _, control_flow| {
        *control_flow = winit::event_loop::ControlFlow::Poll;

        match event {
            winit::event::Event::MainEventsCleared => {
                pixels.lock().unwrap().render().unwrap();
            }
            _ => {
                if input.update(&event) {
                    if input.quit() {
                        *control_flow = winit::event_loop::ControlFlow::Exit;
                        return;
                    }
                }
            }
        }
    });
}
