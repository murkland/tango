use byteorder::{ByteOrder, LittleEndian};
use clap::Parser;
use std::io::Write;

#[derive(clap::Parser)]
struct Cli {
    #[clap(long)]
    dump: bool,

    #[clap(parse(from_os_str))]
    path: Option<std::path::PathBuf>,

    #[clap(parse(from_os_str))]
    output_path: Option<std::path::PathBuf>,

    #[clap(short('a'), long, default_value = "-c:a aac -ar 48000 -b:a 384k -ac 2")]
    ffmpeg_audio_flags: String,

    #[clap(
        short('v'),
        long,
        default_value = "-c:v libx264 -vf scale=iw*5:ih*5:flags=neighbor,format=yuv420p -force_key_frames expr:gte(t,n_forced/2) -crf 18 -bf 2"
    )]
    ffmpeg_video_flags: String,

    #[clap(short('m'), long, default_value = "-movflags +faststart")]
    ffmpeg_mux_flags: String,
}

fn main() -> Result<(), anyhow::Error> {
    env_logger::Builder::from_default_env()
        .filter(Some("tango"), log::LevelFilter::Info)
        .filter(Some("replaydump"), log::LevelFilter::Info)
        .init();
    mgba::log::init();

    let args = Cli::parse();

    let compat_list = std::sync::Arc::new(tango::compat::load()?);

    let path = match args.path {
        Some(path) => path,
        None => native_dialog::FileDialog::new()
            .add_filter("tango replay", &["tangoreplay"])
            .show_open_single_file()?
            .ok_or_else(|| anyhow::anyhow!("no file selected"))?,
    };

    let mut f = std::fs::File::open(path.clone())?;

    let replay = tango::replay::Replay::decode(&mut f)?;
    log::info!(
        "replay is for {} (crc32 = {:08x})",
        replay.state.rom_title(),
        replay.state.rom_crc32()
    );

    let output_path = args
        .output_path
        .unwrap_or_else(|| path.as_path().with_extension("mp4").to_path_buf());

    let (id, rom_path) = std::fs::read_dir("roms")?
        .flat_map(|dirent| {
            let dirent = dirent.as_ref().expect("dirent");
            let mut core = mgba::core::Core::new_gba("tango").expect("new_gba");
            let vf = match mgba::vfile::VFile::open(&dirent.path(), mgba::vfile::flags::O_RDONLY) {
                Ok(vf) => vf,
                Err(e) => {
                    log::warn!(
                        "failed to open {} for probing: {}",
                        dirent.path().display(),
                        e
                    );
                    return vec![];
                }
            };

            if let Err(e) = core.as_mut().load_rom(vf) {
                log::warn!(
                    "failed to load {} for probing: {}",
                    dirent.path().display(),
                    e
                );
                return vec![];
            }

            if core.as_ref().game_title() != replay.state.rom_title() {
                log::warn!(
                    "{} is not eligible (title is {})",
                    dirent.path().display(),
                    core.as_ref().game_title()
                );
                return vec![];
            }

            if core.as_ref().crc32() != replay.state.rom_crc32() {
                log::warn!(
                    "{} is not eligible (crc32 is {:08x})",
                    dirent.path().display(),
                    core.as_ref().crc32()
                );
                return vec![];
            }

            let id = if let Some(id) = compat_list
                .id_by_title_and_crc32(&core.as_ref().game_title(), core.as_ref().crc32())
            {
                id.to_string()
            } else {
                log::warn!(
                    "could not find compatibility data for {} where title = {}, crc32 = {:08x}",
                    dirent.path().display(),
                    core.as_ref().game_title(),
                    core.as_ref().crc32()
                );
                return vec![];
            };

            return vec![(id, dirent.path())];
        })
        .next()
        .ok_or_else(|| anyhow::format_err!("could not find eligible rom"))?;

    log::info!("found rom {}: {}", id, rom_path.display());

    let mut core = mgba::core::Core::new_gba("tango")?;
    core.enable_video_buffer();
    let vf = mgba::vfile::VFile::open(&rom_path, mgba::vfile::flags::O_RDONLY)?;
    core.as_mut().load_rom(vf)?;
    core.as_mut().reset();

    let done = std::sync::Arc::new(parking_lot::Mutex::new(false));

    let ff_state = {
        let done = done.clone();
        tango::fastforwarder::State::new(
            replay.local_player_index,
            replay.input_pairs,
            0,
            0,
            Box::new(move || {
                *done.lock() = true;
            }),
        )
    };
    let hooks = tango::hooks::HOOKS
        .get(&compat_list.game_by_id(&id).unwrap().hooks)
        .unwrap();
    hooks.prepare_for_fastforward(core.as_mut());
    {
        let ff_state = ff_state.clone();
        core.set_traps(hooks.fastforwarder_traps(ff_state));
    }

    core.as_mut().load_state(&replay.state)?;

    let ffmpeg_path = "ffmpeg";

    let video_output = tempfile::NamedTempFile::new()?;
    let mut video_child = std::process::Command::new(&ffmpeg_path)
        .stdin(std::process::Stdio::piped())
        .args(&["-y"])
        // Input args.
        .args(&[
            "-f",
            "rawvideo",
            "-pixel_format",
            "rgba",
            "-video_size",
            "240x160",
            "-framerate",
            "16777216/280896",
            "-i",
            "pipe:",
        ])
        // Output args.
        .args(shell_words::split(&args.ffmpeg_video_flags)?)
        .args(&["-f", "mp4"])
        .arg(&video_output.path())
        .spawn()?;

    let audio_output = tempfile::NamedTempFile::new()?;
    let mut audio_child = std::process::Command::new(&ffmpeg_path)
        .stdin(std::process::Stdio::piped())
        .args(&["-y"])
        // Input args.
        .args(&["-f", "s16le", "-ar", "48k", "-ac", "2", "-i", "pipe:"])
        // Output args.
        .args(shell_words::split(&args.ffmpeg_audio_flags)?)
        .args(&["-f", "mp4"])
        .arg(&audio_output.path())
        .spawn()?;

    const SAMPLE_RATE: f64 = 48000.0;
    let mut samples = vec![0i16; SAMPLE_RATE as usize];
    let mut vbuf = vec![0u8; (mgba::gba::SCREEN_WIDTH * mgba::gba::SCREEN_HEIGHT * 4) as usize];
    let bar = indicatif::ProgressBar::new(ff_state.inputs_pairs_left() as u64);
    while !*done.lock() {
        bar.inc(1);
        core.as_mut().run_frame();
        let clock_rate = core.as_ref().frequency();
        let n = {
            let mut core = core.as_mut();
            let mut left = core.audio_channel(0);
            left.set_rates(clock_rate as f64, SAMPLE_RATE);
            let n = left.samples_avail();
            left.read_samples(&mut samples[..(n * 2) as usize], left.samples_avail(), true);
            n
        };
        {
            let mut core = core.as_mut();
            let mut right = core.audio_channel(1);
            right.set_rates(clock_rate as f64, SAMPLE_RATE);
            right.read_samples(&mut samples[1..(n * 2) as usize], n, true);
        }
        let samples = &samples[..(n * 2) as usize];

        vbuf.copy_from_slice(core.video_buffer().unwrap());
        for i in (0..vbuf.len()).step_by(4) {
            vbuf[i + 3] = 0xff;
        }
        video_child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(vbuf.as_slice())?;

        let mut audio_bytes = vec![0u8; samples.len() * 2];
        LittleEndian::write_i16_into(&samples, &mut audio_bytes[..]);
        audio_child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(&audio_bytes)?;
    }
    bar.finish();

    video_child.stdin = None;
    video_child.wait()?;
    audio_child.stdin = None;
    audio_child.wait()?;

    let mut mux_child = std::process::Command::new(&ffmpeg_path)
        .args(&["-y"])
        .args(&["-i"])
        .arg(video_output.path())
        .args(&["-i"])
        .arg(audio_output.path())
        .args(&["-c:v", "copy", "-c:a", "copy"])
        .args(shell_words::split(&args.ffmpeg_mux_flags)?)
        .arg(output_path)
        .spawn()?;
    mux_child.wait()?;

    Ok(())
}
