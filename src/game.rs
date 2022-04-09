use std::sync::Arc;

use parking_lot::Mutex;

use crate::{audio, battle::Match, bn6, gui, mgba};

pub struct Game {
    main_core: Arc<Mutex<mgba::core::Core>>,
    _trapper: mgba::trapper::Trapper,
    event_loop: Option<winit::event_loop::EventLoop<()>>,
    input: winit_input_helper::WinitInputHelper,
    vbuf: Arc<Vec<u8>>,
    vbuf2: Arc<Mutex<Vec<u8>>>,
    window: winit::window::Window,
    pixels: pixels::Pixels,
    thread: mgba::thread::Thread,
    _stream: rodio::OutputStream,
    gui: gui::Gui,
    r#match: Arc<Mutex<Option<Arc<Match>>>>,
}

impl Game {
    pub fn new() -> Result<Game, Box<dyn std::error::Error>> {
        let main_core = Arc::new(Mutex::new({
            let mut core = mgba::core::Core::new_gba("tango")?;
            core.set_audio_buffer_size(1024);

            let rom_vf = mgba::vfile::VFile::open("bn6f.gba", mgba::vfile::flags::O_RDONLY)?;
            core.load_rom(rom_vf)?;

            let save_vf = mgba::vfile::VFile::open(
                "bn6f.sav",
                mgba::vfile::flags::O_CREAT | mgba::vfile::flags::O_RDWR,
            )?;
            core.load_save(save_vf)?;

            log::info!("loaded game: {}", core.game_title());
            core
        }));

        let event_loop = Some(winit::event_loop::EventLoop::new());

        let (width, height, vbuf, bn6) = {
            let core = main_core.clone();
            let mut core = core.lock();
            let (width, height) = core.desired_video_dimensions();
            let mut vbuf = vec![0u8; (width * height * 4) as usize];
            let bn6 = bn6::BN6::new(&core.game_title());
            core.set_video_buffer(&mut vbuf, width.into());
            (width, height, Arc::new(vbuf), bn6.unwrap())
        };

        let input = winit_input_helper::WinitInputHelper::new();

        let window = {
            let size = winit::dpi::LogicalSize::new(width * 3, height * 3);
            winit::window::WindowBuilder::new()
                .with_title("tango")
                .with_inner_size(size)
                .with_min_inner_size(size)
                .build(event_loop.as_ref().unwrap())?
        };

        let vbuf2 = Arc::new(Mutex::new(vec![0u8; (width * height * 4) as usize]));

        let pixels = {
            let window_size = window.inner_size();
            let surface_texture =
                pixels::SurfaceTexture::new(window_size.width, window_size.height, &window);
            pixels::PixelsBuilder::new(width, height, surface_texture)
                .request_adapter_options(pixels::wgpu::RequestAdapterOptions {
                    power_preference: pixels::wgpu::PowerPreference::HighPerformance,
                    force_fallback_adapter: false,
                    compatible_surface: None,
                })
                .build()?
        };

        let mut thread = {
            let core = main_core.clone();
            mgba::thread::Thread::new(core)
        };
        thread.start();

        let r#match = Arc::new(Mutex::new(None as Option<Arc<Match>>));

        let trapper = {
            let core = main_core.clone();
            let bn6 = bn6.clone();
            let mut core = core.lock();
            mgba::trapper::Trapper::new(
                &mut core,
                vec![
                    {
                        let core = main_core.clone();
                        let r#match = r#match.clone();
                        (
                            bn6.offsets.rom.battle_init_call_battle_copy_input_data,
                            Box::new(move || match &*r#match.lock() {
                                None => {
                                    return;
                                }
                                Some(m) => {
                                    let _ = m.lock_battle().as_ref().expect("attempted to get battle p2 information while no battle was active!");
                                    let mut core = core.lock();
                                    let r15 = core.gba().cpu().gpr(15) as u32;
                                    core.gba_mut().cpu_mut().set_pc(r15 + 4);
                                }
                            }),
                        )
                    },
                    {
                        let core = main_core.clone();
                        (
                            bn6.offsets.rom.battle_init_marshal_ret,
                            Box::new(move || {
                                log::info!("TODO: battle_init_marshal_ret");
                            }),
                        )
                    },
                    {
                        let core = main_core.clone();
                        (
                            bn6.offsets.rom.battle_turn_marshal_ret,
                            Box::new(move || {
                                log::info!("TODO: battle_turn_marshal_ret");
                            }),
                        )
                    },
                    {
                        let core = main_core.clone();
                        (
                            bn6.offsets.rom.main_read_joyflags,
                            Box::new(move || {
                                // log::info!("TODO: main_read_joyflags");
                            }),
                        )
                    },
                    {
                        let core = main_core.clone();
                        let r#match = r#match.clone();
                        let bn6 = bn6.clone();
                        (
                            bn6.offsets.rom.battle_update_call_battle_copy_input_data,
                            Box::new(move || match &*r#match.lock() {
                                None => {
                                    return;
                                }
                                Some(m) => {
                                    let battle = &mut m.lock_battle();
                                    let battle = battle.as_mut().expect("attempted to get battle p2 information while no battle was active!");
                                    let mut core = core.lock();
                                    let r15 = core.gba().cpu().gpr(15) as u32;
                                    core.gba_mut().cpu_mut().set_pc(r15 + 4);

                                    battle.start_accepting_input();

                                    let ip = battle.take_last_input().unwrap();

                                    bn6.set_player_input_state(
                                        &mut core,
                                        0,
                                        ip[0].joyflags as u16,
                                        ip[0].custom_screen_state as u8,
                                    );
                                    if let Some(turn) = ip[0].turn {
                                        bn6.set_player_marshaled_battle_state(&mut core, 0, &turn);
                                    }
                                    bn6.set_player_input_state(
                                        &mut core,
                                        1,
                                        ip[1].joyflags as u16,
                                        ip[1].custom_screen_state as u8,
                                    );
                                    if let Some(turn) = ip[1].turn {
                                        bn6.set_player_marshaled_battle_state(&mut core, 1, &turn);
                                    }
                                }
                            }),
                        )
                    },
                    {
                        let core = main_core.clone();
                        (
                            bn6.offsets.rom.battle_run_unpaused_step_cmp_retval,
                            Box::new(move || {
                                log::info!("TODO: battle_run_unpaused_step_cmp_retval");
                            }),
                        )
                    },
                    {
                        let core = main_core.clone();
                        (
                            bn6.offsets.rom.battle_start_ret,
                            Box::new(move || {
                                log::info!("TODO: battle_start_ret");
                            }),
                        )
                    },
                    {
                        let core = main_core.clone();
                        (
                            bn6.offsets.rom.battle_ending_ret,
                            Box::new(move || {
                                log::info!("TODO: battle_ending_ret");
                            }),
                        )
                    },
                    {
                        let core = main_core.clone();
                        let r#match = r#match.clone();
                        (
                            bn6.offsets.rom.battle_is_p2_tst,
                            Box::new(move || {
                                match &*r#match.lock() {
                                    None => {
                                        return;
                                    }
                                    Some(m) => {
                                        let battle = m.lock_battle();
                                        core.lock()
                                        .gba_mut()
                                        .cpu_mut()
                                        .set_gpr(0, battle.as_ref().expect("attempted to get battle p2 information while no battle was active!").local_player_index() as i32);
                                    }
                                };
                            }),
                        )
                    },
                    {
                        let core = main_core.clone();
                        let r#match = r#match.clone();
                        (
                            bn6.offsets.rom.link_is_p2_ret,
                            Box::new(move || {
                                match &*r#match.lock() {
                                    None => {
                                        return;
                                    }
                                    Some(m) => {
                                        let battle = m.lock_battle();
                                        core.lock()
                                        .gba_mut()
                                        .cpu_mut()
                                        .set_gpr(0, battle.as_ref().expect("attempted to get battle p2 information while no battle was active!").local_player_index() as i32);
                                    }
                                };
                            }),
                        )
                    },
                    {
                        let core = main_core.clone();
                        (
                            bn6.offsets.rom.battle_start_ret,
                            Box::new(move || {
                                log::info!("TODO: battle_start_ret");
                            }),
                        )
                    },
                    {
                        let core = main_core.clone();
                        (
                            bn6.offsets.rom.get_copy_data_input_state_ret,
                            Box::new(move || {
                                log::info!("TODO: get_copy_data_input_state_ret");
                            }),
                        )
                    },
                    {
                        let core = main_core.clone();
                        (
                            bn6.offsets.rom.comm_menu_handle_link_cable_input_entry,
                            Box::new(move || {
                                let core = core.lock();
                                log::warn!("unhandled call to commMenu_handleLinkCableInput at 0x{:0x}: uh oh!", core.gba().cpu().gpr(15)-4);
                            }),
                        )
                    },
                    {
                        let core = main_core.clone();
                        let r#match = r#match.clone();
                        let bn6 = bn6.clone();
                        (
                            bn6.offsets
                                .rom
                                .comm_menu_wait_for_friend_call_comm_menu_handle_link_cable_input,
                            Box::new(move || {
                                let mut core = core.lock();
                                let r15 = core.gba().cpu().gpr(15) as u32;
                                core.gba_mut().cpu_mut().set_pc(r15 + 4);

                                let r#match = r#match.clone();
                                let mut r#match = r#match.lock();
                                match &*r#match {
                                    None => {
                                        let m = Arc::new(Match::new(
                                            "test".to_string(),
                                            bn6.match_type(&core),
                                            core.game_title(),
                                            core.crc32(),
                                        ));
                                        *r#match = Some(m.clone());
                                        std::thread::spawn(move || {
                                            if let Err(e) = m.run() {
                                                log::info!("match ended with {}", e);
                                            } else {
                                                log::info!("match ended with ok");
                                            }
                                        });
                                    }
                                    Some(r#match) => match r#match.poll_for_ready() {
                                        Ok(true) => {
                                            bn6.start_battle_from_comm_menu(&mut core);
                                            log::info!("match started");
                                        }
                                        Ok(false) => {}
                                        Err(err) => {
                                            // TODO: return the correct error.
                                            bn6.drop_matchmaking_from_comm_menu(&mut core, 0);
                                        }
                                    },
                                };
                            }),
                        )
                    },
                    {
                        let core = main_core.clone();
                        (
                            bn6.offsets.rom.comm_menu_init_battle_entry,
                            Box::new(move || {
                                log::info!("TODO: comm_menu_init_battle_entry");
                            }),
                        )
                    },
                    {
                        let core = main_core.clone();
                        let r#match = r#match.clone();
                        (
                            bn6.offsets.rom.comm_menu_wait_for_friend_ret_cancel,
                            Box::new(move || {
                                log::info!("match canceled by user");
                                let mut r#match = r#match.lock();
                                r#match.as_ref().unwrap().cancel();
                                *r#match = None;
                                let mut core = core.lock();
                                let r15 = core.gba().cpu().gpr(15) as u32;
                                core.gba_mut().cpu_mut().set_pc(r15 + 4);
                            }),
                        )
                    },
                    {
                        let core = main_core.clone();
                        (
                            bn6.offsets.rom.comm_menu_end_battle_entry,
                            Box::new(move || {
                                log::info!("TODO: comm_menu_end_battle_entry");
                            }),
                        )
                    },
                    {
                        let core = main_core.clone();
                        (
                            bn6.offsets.rom.comm_menu_handle_link_cable_input_entry,
                            Box::new(move || {
                                let mut core = core.lock();
                                let r15 = core.gba().cpu().gpr(15) as u32;
                                core.gba_mut().cpu_mut().set_pc(r15 + 4);
                            }),
                        )
                    },
                    {
                        let core = main_core.clone();
                        (
                            bn6.offsets
                                .rom
                                .comm_menu_in_battle_call_comm_menu_handle_link_cable_input,
                            Box::new(move || {
                                let mut core = core.lock();
                                let r15 = core.gba().cpu().gpr(15) as u32;
                                core.gba_mut().cpu_mut().set_pc(r15 + 4);
                            }),
                        )
                    },
                ],
            )
        };

        let (stream, stream_handle) = rodio::OutputStream::try_default().unwrap();
        let audio_source = {
            let core = main_core.clone();
            audio::MGBAAudioSource::new(core, 48000)
        };
        stream_handle.play_raw(audio_source)?;

        {
            let core = main_core.clone();
            let mut core = core.lock();
            core.gba_mut()
                .sync_mut()
                .as_mut()
                .unwrap()
                .set_fps_target(60.0);
        }

        let gui = gui::Gui::new(&window, &pixels);

        let mut game = Game {
            main_core,
            _trapper: trapper,
            event_loop,
            input,
            window,
            pixels,
            vbuf,
            vbuf2,
            thread,
            _stream: stream,
            gui,
            r#match,
        };

        {
            let vbuf = Arc::clone(&game.vbuf);
            let vbuf2 = Arc::clone(&game.vbuf2);
            game.thread.set_frame_callback(Some(Box::new(move || {
                let mut vbuf2 = vbuf2.lock();
                vbuf2.copy_from_slice(&vbuf);
                for i in (0..vbuf2.len()).step_by(4) {
                    vbuf2[i + 3] = 0xff;
                }
            })));
        }

        Ok(game)
    }

    pub fn run(mut self: Self) {
        self.event_loop
            .take()
            .unwrap()
            .run(move |event, _, control_flow| {
                *control_flow = winit::event_loop::ControlFlow::Poll;

                if let winit::event::Event::RedrawRequested(_) = event {
                    {
                        let vbuf2 = self.vbuf2.lock().clone();
                        self.pixels.get_frame().copy_from_slice(&vbuf2);
                    }

                    self.gui
                        .prepare(&self.window)
                        .expect("gui.prepare() failed");
                    self.pixels
                        .render_with(|encoder, render_target, context| {
                            context.scaling_renderer.render(encoder, render_target);
                            self.gui
                                .render(&self.window, encoder, render_target, context)?;
                            Ok(())
                        })
                        .unwrap();
                }

                self.gui.handle_event(&self.window, &event);
                if self.input.update(&event) {
                    if self.input.quit() {
                        *control_flow = winit::event_loop::ControlFlow::Exit;
                        return;
                    }

                    if let Some(size) = self.input.window_resized() {
                        self.pixels.resize_surface(size.width, size.height);
                    }

                    let mut core = self.main_core.lock();

                    let mut keys = 0u32;
                    if self.input.key_held(winit::event::VirtualKeyCode::Left) {
                        keys |= mgba::input::keys::LEFT;
                    }
                    if self.input.key_held(winit::event::VirtualKeyCode::Right) {
                        keys |= mgba::input::keys::RIGHT;
                    }
                    if self.input.key_held(winit::event::VirtualKeyCode::Up) {
                        keys |= mgba::input::keys::UP;
                    }
                    if self.input.key_held(winit::event::VirtualKeyCode::Down) {
                        keys |= mgba::input::keys::DOWN;
                    }
                    if self.input.key_held(winit::event::VirtualKeyCode::Z) {
                        keys |= mgba::input::keys::A;
                    }
                    if self.input.key_held(winit::event::VirtualKeyCode::X) {
                        keys |= mgba::input::keys::B;
                    }
                    if self.input.key_held(winit::event::VirtualKeyCode::A) {
                        keys |= mgba::input::keys::L;
                    }
                    if self.input.key_held(winit::event::VirtualKeyCode::S) {
                        keys |= mgba::input::keys::R;
                    }
                    if self.input.key_held(winit::event::VirtualKeyCode::Return) {
                        keys |= mgba::input::keys::START;
                    }
                    if self.input.key_held(winit::event::VirtualKeyCode::Back) {
                        keys |= mgba::input::keys::SELECT;
                    }

                    core.set_keys(keys);

                    self.window.request_redraw();
                }
            });
    }
}
