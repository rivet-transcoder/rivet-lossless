//! FLAC and ALAC against independent implementations, used as black boxes.
//!
//! Decode: synthetic PCM → the `flac` command-line encoder or ffmpeg's ALAC
//! encoder → this crate's decoder, which must give back the PCM exactly.
//! Encode: synthetic PCM → this crate's encoder → `flac -d` and ffmpeg,
//! which must give back the PCM exactly.
//!
//! Each test SKIPs (passes, printing why) when the tool it needs is not on
//! PATH, so the suite runs anywhere — unless `RIVET_REQUIRE_LOSSLESS_ORACLES`
//! is set, as CI sets it, where a missing tool is a failure.
//!
//! The files go in and out through the few container pieces at the end of
//! this file — a native FLAC stream, the boxes of an MP4 audio track, the
//! elements of a Matroska one — just enough to reach the packets and the
//! codec configuration.

use std::path::{Path, PathBuf};
use std::process::Command;

use lossless::flac::{EncoderConfig as FlacEncoderConfig, Level as FlacLevel};
use lossless::{alac, flac};

fn have(tool: &str) -> bool {
    let arg = if tool == "flac" { "--version" } else { "-version" };
    let found = Command::new(tool).arg(arg).output().is_ok_and(|o| o.status.success());
    if !found && std::env::var_os("RIVET_REQUIRE_LOSSLESS_ORACLES").is_some() {
        panic!("RIVET_REQUIRE_LOSSLESS_ORACLES is set, and `{tool}` is not on PATH");
    }
    found
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rivet-lossless-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn run(cmd: &mut Command) {
    let out = cmd.output().expect("spawn");
    assert!(out.status.success(), "{cmd:?} failed: {}", String::from_utf8_lossy(&out.stderr));
}

/// Deterministic test audio: per channel, a mix of a few tones and noise,
/// the channels partly correlated (so every stereo mode has something to
/// win), a stretch of digital silence, a stretch of a constant, and a burst
/// at full scale.
fn signal(frames: usize, channels: usize, bits: u32, seed: u32) -> Vec<i32> {
    let full = f64::from((1u32 << (bits - 1)) - 1);
    let mut rng = seed.wrapping_mul(2_654_435_761).max(1);
    let mut noise = move || {
        rng ^= rng << 13;
        rng ^= rng >> 17;
        rng ^= rng << 5;
        f64::from(rng) / f64::from(u32::MAX) - 0.5
    };
    let mut out = Vec::with_capacity(frames * channels);
    for i in 0..frames {
        let t = i as f64 / 48_000.0;
        let common = (t * 2.0 * std::f64::consts::PI * 220.0).sin() * 0.4
            + (t * 2.0 * std::f64::consts::PI * 1_375.0).sin() * 0.1;
        let n = noise();
        for c in 0..channels {
            let own = (t * 2.0 * std::f64::consts::PI * (330.0 + 110.0 * c as f64)).sin() * 0.2;
            let v = if i > frames / 3 && i < frames / 3 + 5_000 {
                0.0
            } else if i >= frames / 2 && i < frames / 2 + 3_000 {
                0.25
            } else if i >= frames * 3 / 4 && i < frames * 3 / 4 + 500 {
                if (i / 7 + c) % 2 == 0 { 1.0 } else { -1.0 }
            } else {
                common + own * (c as f64 * 0.3) + n * 0.05 + noise() * 0.02
            };
            let s = (v * full).round().clamp(-full - 1.0, full) as i64;
            out.push(s as i32);
        }
    }
    out
}

/// Little-endian raw PCM in the fewest whole bytes per sample.
fn raw_bytes(samples: &[i32], bits: u32) -> Vec<u8> {
    let width = bits.div_ceil(8) as usize;
    samples.iter().flat_map(|s| s.to_le_bytes()[..width].to_vec()).collect()
}

fn read_raw(path: &Path, bits: u32) -> Vec<i32> {
    let width = bits.div_ceil(8) as usize;
    std::fs::read(path)
        .unwrap()
        .chunks_exact(width)
        .map(|b| {
            let mut v = [0u8; 4];
            v[4 - width..].copy_from_slice(b);
            i32::from_le_bytes(v) >> (32 - 8 * width)
        })
        .collect()
}

fn decode_flac_track(track: &Track) -> (Vec<i32>, flac::Decoder) {
    let mut dec = flac::Decoder::new(Some(&track.config), 0, 0).unwrap();
    let mut out = Vec::new();
    for p in &track.packets {
        out.extend(dec.decode_int(p).unwrap().0);
    }
    (out, dec)
}

fn decode_alac_track(track: &Track) -> Vec<i32> {
    let mut dec = alac::Decoder::new(Some(&track.config)).unwrap();
    let mut out = Vec::new();
    for p in &track.packets {
        out.extend(dec.decode_int(p).unwrap());
    }
    out
}

fn first_mismatch(a: &[i32], b: &[i32]) -> String {
    match a.iter().zip(b).position(|(x, y)| x != y) {
        Some(i) => format!("first difference at sample {i}: got {} want {}", a[i], b[i]),
        None => format!("lengths differ: got {} want {}", a.len(), b.len()),
    }
}

#[test]
fn flac_cli_streams_decode_bit_exact() {
    if !have("flac") {
        eprintln!("SKIP: no `flac` on PATH");
        return;
    }
    let cases: &[(u32, usize, u32, &[&str])] = &[
        (44_100, 2, 16, &["-5"]),
        (44_100, 2, 16, &["-0"]),
        (44_100, 2, 16, &["-8"]),
        (48_000, 1, 16, &["-5"]),
        (48_000, 2, 24, &["-8"]),
        (96_000, 2, 24, &["-5", "--no-mid-side"]),
        (48_000, 6, 16, &["-5"]),
        (96_000, 6, 24, &["-8"]),
        (48_000, 8, 24, &["-5"]),
        (44_100, 2, 8, &["-5"]),
        (48_000, 2, 32, &["-5"]),
        (44_100, 2, 16, &["-5", "-b", "1152"]),
        (22_050, 3, 16, &["-3"]),
        (32_000, 5, 16, &["-5"]),
        (48_000, 7, 16, &["-5"]),
        (48_000, 4, 16, &["-5", "-b", "576"]),
        (192_000, 2, 24, &["-8", "-l", "32"]),
    ];
    for (i, &(rate, channels, bits, args)) in cases.iter().enumerate() {
        let pcm = signal(rate as usize * 3 / 2 + 1_234, channels, bits, i as u32 + 1);
        let raw = scratch(&format!("f{i}.raw"));
        let flac = scratch(&format!("f{i}.flac"));
        std::fs::write(&raw, raw_bytes(&pcm, bits)).unwrap();
        run(Command::new("flac")
            .args(["--silent", "-f", "--force-raw-format", "--endian=little", "--sign=signed"])
            .arg(format!("--channels={channels}"))
            .arg(format!("--bps={bits}"))
            .arg(format!("--sample-rate={rate}"))
            .args(args)
            .arg("-o")
            .arg(&flac)
            .arg(&raw));
        let data = std::fs::read(&flac).unwrap();
        let src = demux_audio(&data);
        assert_eq!(src.codec, "flac");
        let (got, dec) = decode_flac_track(&src);
        let label = format!("{rate} Hz {channels}ch {bits}-bit flac {args:?}");
        assert!(got == pcm, "{label}: {}", first_mismatch(&got, &pcm));
        assert_eq!(dec.md5_matches(), Some(true), "{label}: MD5");
        assert_eq!(dec.samples_decoded() as usize, pcm.len() / channels);
        eprintln!("ok: {label}");
    }
}

#[test]
fn flac_in_mp4_and_matroska_decodes_bit_exact() {
    if !have("flac") || !have("ffmpeg") {
        eprintln!("SKIP: needs `flac` and `ffmpeg` on PATH");
        return;
    }
    for (i, &(rate, channels, bits)) in [(48_000u32, 2usize, 16u32), (96_000, 6, 24)].iter().enumerate() {
        let pcm = signal(rate as usize + 777, channels, bits, 40 + i as u32);
        let raw = scratch(&format!("m{i}.raw"));
        let flac = scratch(&format!("m{i}.flac"));
        std::fs::write(&raw, raw_bytes(&pcm, bits)).unwrap();
        run(Command::new("flac")
            .args(["--silent", "-f", "--force-raw-format", "--endian=little", "--sign=signed"])
            .arg(format!("--channels={channels}"))
            .arg(format!("--bps={bits}"))
            .arg(format!("--sample-rate={rate}"))
            .arg("-o")
            .arg(&flac)
            .arg(&raw));
        for ext in ["mp4", "mkv"] {
            let out = scratch(&format!("m{i}.{ext}"));
            run(Command::new("ffmpeg")
                .args(["-loglevel", "error", "-y", "-i"])
                .arg(&flac)
                .args(["-c:a", "copy", "-strict", "-2"])
                .arg(&out));
            let src = demux_audio(&std::fs::read(&out).unwrap());
            assert_eq!(src.codec, "flac", "{ext}");
            let (got, dec) = decode_flac_track(&src);
            assert!(got == pcm, "FLAC in {ext} {channels}ch {bits}-bit: {}", first_mismatch(&got, &pcm));
            assert_eq!(dec.samples_decoded(), (pcm.len() / channels) as u64);
            eprintln!("ok: FLAC in {ext} {rate} Hz {channels}ch {bits}-bit");
        }
    }
}

#[test]
fn ffmpeg_alac_decodes_bit_exact() {
    if !have("ffmpeg") {
        eprintln!("SKIP: no `ffmpeg` on PATH");
        return;
    }
    let cases: &[(u32, usize, u32)] = &[
        (44_100, 2, 16),
        (44_100, 1, 16),
        (48_000, 2, 24),
        (96_000, 2, 24),
        (48_000, 6, 16),
        (96_000, 6, 24),
        (48_000, 8, 16),
        (48_000, 3, 16),
        (48_000, 4, 16),
        (48_000, 5, 24),
        (48_000, 7, 16),
    ];
    for (i, &(rate, channels, bits)) in cases.iter().enumerate() {
        let pcm = signal(rate as usize * 3 / 2 + 99, channels, bits, 80 + i as u32);
        let raw = scratch(&format!("a{i}.raw"));
        std::fs::write(&raw, raw_bytes(&pcm, bits)).unwrap();
        let fmt = if bits == 16 { "s16le" } else { "s24le" };
        for ext in ["m4a", "mkv"] {
            let out = scratch(&format!("a{i}.{ext}"));
            let mut cmd = Command::new("ffmpeg");
            cmd.args(["-loglevel", "error", "-y", "-f", fmt, "-ar", &rate.to_string(), "-ac", &channels.to_string()]);
            // Name the layouts ALAC has for these counts: given ffmpeg's
            // defaults (2.1, quad, 7.1) its encoder silently remixes.
            if let Some(layout) = match channels {
                3 => Some("3.0"),
                4 => Some("4.0"),
                5 => Some("5.0"),
                8 => Some("7.1(wide)"),
                _ => None,
            } {
                cmd.args(["-channel_layout", layout]);
            }
            run(cmd
                .arg("-i")
                .arg(&raw)
                .args(["-c:a", "alac"])
                .arg(&out));
            let src = demux_audio(&std::fs::read(&out).unwrap());
            assert_eq!(src.codec, "alac", "{ext}");
            let got = decode_alac_track(&src);
            let label = format!("ALAC in {ext} {rate} Hz {channels}ch {bits}-bit");
            assert!(got == pcm, "{label}: {}", first_mismatch(&got, &pcm));
            eprintln!("ok: {label}");
        }
    }
}

fn rivet_flac(pcm: &[i32], rate: u32, channels: u8, bits: u8, level: FlacLevel) -> (Vec<u8>, Vec<(Vec<u8>, u32)>) {
    let mut enc =
        flac::Encoder::new(FlacEncoderConfig { sample_rate: rate, channels, bits_per_sample: bits, level }).unwrap();
    let mut frames = enc.encode_int(pcm);
    frames.extend(enc.finish());
    (enc.metadata_blocks(), frames)
}

fn rivet_alac(pcm: &[i32], rate: u32, channels: u8, bits: u8) -> (Vec<u8>, Vec<(Vec<u8>, u32)>) {
    let mut enc = alac::Encoder::new(rate, channels, bits).unwrap();
    let mut frames = enc.encode_int(pcm);
    frames.extend(enc.finish());
    (enc.cookie().to_bytes().to_vec(), frames)
}

/// ffmpeg's decode of `path` as raw PCM at `bits`.
fn ffmpeg_decode(path: &Path, bits: u32) -> Vec<i32> {
    let out = path.with_extension("ffdec.raw");
    let fmt = match bits {
        16 => "s16le",
        24 => "s24le",
        _ => "s32le",
    };
    run(Command::new("ffmpeg")
        .args(["-loglevel", "error", "-y", "-i"])
        .arg(path)
        .args(["-c:a", &format!("pcm_{fmt}"), "-f", fmt])
        .arg(&out));
    read_raw(&out, bits)
}

#[test]
fn rivet_flac_decodes_bit_exact_in_flac_and_ffmpeg() {
    if !have("flac") || !have("ffmpeg") {
        eprintln!("SKIP: needs `flac` and `ffmpeg` on PATH");
        return;
    }
    let cases: &[(u32, u8, u8, FlacLevel)] = &[
        (44_100, 2, 16, FlacLevel::Fast),
        (44_100, 2, 16, FlacLevel::Default),
        (44_100, 2, 16, FlacLevel::Best),
        (48_000, 1, 16, FlacLevel::Default),
        (48_000, 2, 24, FlacLevel::Default),
        (96_000, 2, 24, FlacLevel::Best),
        (48_000, 6, 16, FlacLevel::Default),
        (96_000, 6, 24, FlacLevel::Default),
        (48_000, 8, 24, FlacLevel::Default),
        (192_000, 2, 24, FlacLevel::Fast),
        (48_000, 2, 32, FlacLevel::Default),
        (22_050, 3, 16, FlacLevel::Default),
    ];
    for (i, &(rate, channels, bits, level)) in cases.iter().enumerate() {
        let pcm = signal(rate as usize * 3 / 2 + 555, usize::from(channels), u32::from(bits), 200 + i as u32);
        let (blocks, frames) = rivet_flac(&pcm, rate, channels, bits, level);
        let label = format!("rivet FLAC {rate} Hz {channels}ch {bits}-bit {level:?}");
        let native = scratch(&format!("e{i}.flac"));
        std::fs::write(&native, native_flac(&blocks, &frames)).unwrap();
        // The reference decoder, with its MD5 check.
        run(Command::new("flac").args(["--silent", "-t"]).arg(&native));
        let raw = scratch(&format!("e{i}.dec.raw"));
        run(Command::new("flac")
            .args(["--silent", "-f", "-d", "--force-raw-format", "--endian=little", "--sign=signed", "-o"])
            .arg(&raw)
            .arg(&native));
        let got = read_raw(&raw, u32::from(bits));
        assert!(got == pcm, "{label} via flac -d: {}", first_mismatch(&got, &pcm));
        if bits != 32 {
            let got = ffmpeg_decode(&native, u32::from(bits));
            assert!(got == pcm, "{label} via ffmpeg: {}", first_mismatch(&got, &pcm));
            // FLAC in MP4.
            let mp4 = scratch(&format!("e{i}.mp4"));
            std::fs::write(&mp4, m4a(Entry::Flac(&blocks), rate, channels, bits, &frames)).unwrap();
            let got = ffmpeg_decode(&mp4, u32::from(bits));
            assert!(got == pcm, "{label} in MP4 via ffmpeg: {}", first_mismatch(&got, &pcm));
        }
        eprintln!("ok: {label}");
    }
}

#[test]
fn rivet_alac_decodes_bit_exact_in_ffmpeg() {
    if !have("ffmpeg") {
        eprintln!("SKIP: no `ffmpeg` on PATH");
        return;
    }
    let cases: &[(u32, u8, u8)] = &[
        (44_100, 2, 16),
        (44_100, 1, 16),
        (48_000, 2, 24),
        (96_000, 2, 24),
        (48_000, 6, 16),
        (96_000, 6, 24),
        (48_000, 8, 16),
        (48_000, 3, 16),
        (48_000, 4, 24),
        (48_000, 5, 16),
        (48_000, 7, 16),
        (48_000, 2, 20),
        (48_000, 2, 32),
    ];
    for (i, &(rate, channels, bits)) in cases.iter().enumerate() {
        let pcm = signal(rate as usize * 3 / 2 + 321, usize::from(channels), u32::from(bits), 300 + i as u32);
        let (cookie, frames) = rivet_alac(&pcm, rate, channels, bits);
        let m4a_path = scratch(&format!("r{i}.m4a"));
        std::fs::write(&m4a_path, m4a(Entry::Alac(&cookie), rate, channels, bits, &frames)).unwrap();
        // 20-bit audio comes out of ffmpeg left-justified in 24 bits.
        let (out_bits, shift) = match bits {
            20 => (24, 4),
            b => (u32::from(b), 0),
        };
        let mut got: Vec<i32> = ffmpeg_decode(&m4a_path, out_bits).into_iter().map(|s| s >> shift).collect();
        // ffmpeg names ALAC's seven channels 6.1(back) (… BL BR BC); the
        // pipeline's 6.1 is FL FR FC LFE BC SL SR.
        if channels == 7 {
            for f in got.as_chunks_mut::<7>().0 {
                let (ls, rs, cs) = (f[4], f[5], f[6]);
                f[4..7].copy_from_slice(&[cs, ls, rs]);
            }
        }
        let label = format!("rivet ALAC {rate} Hz {channels}ch {bits}-bit");
        assert!(got == pcm, "{label} via ffmpeg: {}", first_mismatch(&got, &pcm));
        eprintln!("ok: {label}");
    }
}

/// Sizes against the reference encoders on a few synthetic signals, for
/// the record: printed, with nothing asserted beyond beating raw PCM.
#[test]
fn compression_against_the_reference_encoders() {
    if !have("flac") || !have("ffmpeg") {
        eprintln!("SKIP: needs `flac` and `ffmpeg` on PATH");
        return;
    }
    let rate = 44_100u32;
    let frames = rate as usize * 10;
    let mut rng = 99u32;
    let mut noise = move || {
        rng ^= rng << 13;
        rng ^= rng >> 17;
        rng ^= rng << 5;
        f64::from(rng) / f64::from(u32::MAX) - 0.5
    };
    let mut brown = [0.0f64; 2];
    let sine: Vec<i32> = (0..frames)
        .flat_map(|i| {
            let v = ((i as f64 / 44_100.0 * 2.0 * std::f64::consts::PI * 1000.0).sin() * 20_000.0) as i32;
            [v, v / 2]
        })
        .collect();
    let brownian: Vec<i32> = (0..frames)
        .flat_map(|_| {
            let mut out = [0i32; 2];
            for (c, p) in brown.iter_mut().enumerate() {
                *p = (*p * 0.995 + noise() * 800.0).clamp(-32_000.0, 32_000.0);
                out[c] = *p as i32;
            }
            out
        })
        .collect();
    let signals: Vec<(&str, u32, Vec<i32>)> = vec![
        ("tones+noise 16-bit", 16, signal(frames, 2, 16, 1)),
        ("tones+noise 24-bit", 24, signal(frames, 2, 24, 2)),
        ("sine 1 kHz 16-bit", 16, sine),
        ("brown noise 16-bit", 16, brownian),
    ];
    eprintln!(
        "| signal | PCM bytes | rivet FLAC fast | rivet FLAC default | rivet FLAC best | flac -5 | rivet ALAC | ffmpeg ALAC |"
    );
    for (i, (name, bits, pcm)) in signals.iter().enumerate() {
        let raw_len = pcm.len() * (*bits as usize / 8);
        let raw = scratch(&format!("c{i}.raw"));
        std::fs::write(&raw, raw_bytes(pcm, *bits)).unwrap();
        let rivet: Vec<usize> = [FlacLevel::Fast, FlacLevel::Default, FlacLevel::Best]
            .iter()
            .map(|&l| {
                let (blocks, frames) = rivet_flac(pcm, rate, 2, *bits as u8, l);
                native_flac(&blocks, &frames).len()
            })
            .collect();
        let reference = scratch(&format!("c{i}.flac"));
        run(Command::new("flac")
            .args(["--silent", "-f", "-5", "--force-raw-format", "--endian=little", "--sign=signed", "--channels=2"])
            .arg(format!("--bps={bits}"))
            .arg(format!("--sample-rate={rate}"))
            .arg("-o")
            .arg(&reference)
            .arg(&raw));
        let flac5 = std::fs::metadata(&reference).unwrap().len() as usize;
        let (cookie, frames_a) = rivet_alac(pcm, rate, 2, *bits as u8);
        let rivet_alac_len = m4a(Entry::Alac(&cookie), rate, 2, *bits as u8, &frames_a).len();
        let ff = scratch(&format!("c{i}.m4a"));
        let fmt = if *bits == 16 { "s16le" } else { "s24le" };
        run(Command::new("ffmpeg")
            .args(["-loglevel", "error", "-y", "-f", fmt, "-ar", &rate.to_string(), "-ac", "2", "-i"])
            .arg(&raw)
            .args(["-c:a", "alac"])
            .arg(&ff));
        let ff_alac = std::fs::metadata(&ff).unwrap().len() as usize;
        let pct = |n: usize| format!("{:.1}%", 100.0 * n as f64 / raw_len as f64);
        eprintln!(
            "| {name} | {raw_len} | {} | {} | {} | {} | {} | {} |",
            pct(rivet[0]),
            pct(rivet[1]),
            pct(rivet[2]),
            pct(flac5),
            pct(rivet_alac_len),
            pct(ff_alac)
        );
        assert!(rivet.iter().all(|&n| n < raw_len) && rivet_alac_len < raw_len + raw_len / 50);
    }
}

// ---------------------------------------------------------------------------
// Containers: just enough of each to reach the packets and the codec
// configuration, and to wrap this crate's output for the reference tools.
// ---------------------------------------------------------------------------

/// What the tests need of a file's one audio track.
struct Track {
    codec: &'static str,
    /// The codec configuration as the container holds it: the native
    /// stream's head (`fLaC` + metadata blocks), a `dfLa` or `alac` box's
    /// body, or Matroska CodecPrivate.
    config: Vec<u8>,
    packets: Vec<Vec<u8>>,
}

fn demux_audio(data: &[u8]) -> Track {
    if data.starts_with(b"fLaC") {
        read_native_flac(data)
    } else if data.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        read_matroska(data)
    } else {
        read_mp4(data)
    }
}

/// A native FLAC stream: `fLaC`, the metadata blocks, then the frames back
/// to back — handed over as one packet, which the decoder takes frame by
/// frame.
fn read_native_flac(data: &[u8]) -> Track {
    let (_, blocks) = flac::parse_metadata_blocks(&data[4..]).unwrap();
    let head = 4 + blocks;
    Track { codec: "flac", config: data[..head].to_vec(), packets: vec![data[head..].to_vec()] }
}

/// `fLaC`, STREAMINFO (alone, flagged last, as the encoder gives it) and
/// the frames.
fn native_flac(blocks: &[u8], frames: &[(Vec<u8>, u32)]) -> Vec<u8> {
    let mut out = b"fLaC".to_vec();
    out.extend_from_slice(blocks);
    for (f, _) in frames {
        out.extend_from_slice(f);
    }
    out
}

fn be32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(b[at..at + 4].try_into().unwrap())
}

/// The boxes directly inside `data` (ISO/IEC 14496-12): `(type, body)`.
fn boxes(data: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 8 <= data.len() {
        let size = be32(data, i) as usize;
        let kind: [u8; 4] = data[i + 4..i + 8].try_into().unwrap();
        let (header, size) = match size {
            1 => (16, u64::from_be_bytes(data[i + 8..i + 16].try_into().unwrap()) as usize),
            0 => (8, data.len() - i),
            n => (8, n),
        };
        out.push((kind, &data[i + header..i + size]));
        i += size;
    }
    out
}

fn child<'a>(data: &'a [u8], path: &[&[u8; 4]]) -> &'a [u8] {
    path.iter().fold(data, |d, want| {
        boxes(d).into_iter().find(|(k, _)| k == *want).unwrap_or_else(|| panic!("no {want:?} box")).1
    })
}

/// The first track of an MP4: its sample entry's configuration box body
/// and its samples, through `stsz`, `stsc` and `stco` / `co64`.
fn read_mp4(data: &[u8]) -> Track {
    let stbl = child(data, &[b"moov", b"trak", b"mdia", b"minf", b"stbl"]);
    let stsd = child(stbl, &[b"stsd"]);
    let (kind, entry) = boxes(&stsd[8..])[0];
    // An AudioSampleEntry's fields take 28 bytes; QuickTime's sound
    // descriptions of version 1 and 2 take 44 and 64.
    let head = match u16::from_be_bytes([entry[8], entry[9]]) {
        1 => 44,
        2 => 64,
        _ => 28,
    };
    let (codec, want) = match &kind {
        b"fLaC" => ("flac", b"dfLa"),
        b"alac" => ("alac", b"alac"),
        k => panic!("sample entry {:?}", String::from_utf8_lossy(k)),
    };
    let config = boxes(&entry[head..]).into_iter().find(|(k, _)| k == want).expect("the configuration box").1;

    let stsz = child(stbl, &[b"stsz"]);
    let count = be32(stsz, 8) as usize;
    let sizes: Vec<usize> = match be32(stsz, 4) {
        0 => (0..count).map(|i| be32(stsz, 12 + 4 * i) as usize).collect(),
        fixed => vec![fixed as usize; count],
    };
    let tables = boxes(stbl);
    let offsets: Vec<u64> = if let Some((_, co)) = tables.iter().find(|(k, _)| k == b"stco") {
        (0..be32(co, 4) as usize).map(|i| u64::from(be32(co, 8 + 4 * i))).collect()
    } else {
        let co = child(stbl, &[b"co64"]);
        (0..be32(co, 4) as usize).map(|i| u64::from_be_bytes(co[8 + 8 * i..16 + 8 * i].try_into().unwrap())).collect()
    };
    let stsc = child(stbl, &[b"stsc"]);
    let runs: Vec<(usize, usize)> =
        (0..be32(stsc, 4) as usize).map(|i| (be32(stsc, 8 + 12 * i) as usize, be32(stsc, 12 + 12 * i) as usize)).collect();
    let mut packets = Vec::with_capacity(count);
    let mut sample = 0;
    for (chunk, &offset) in offsets.iter().enumerate() {
        let per = runs.iter().rev().find(|(first, _)| *first <= chunk + 1).expect("an stsc run").1;
        let mut at = offset as usize;
        for _ in 0..per.min(count - sample) {
            packets.push(data[at..at + sizes[sample]].to_vec());
            at += sizes[sample];
            sample += 1;
        }
    }
    assert_eq!(packets.len(), count, "every sample placed");
    Track { codec, config: config.to_vec(), packets }
}

/// What an `.m4a` written by [`m4a`] carries.
enum Entry<'a> {
    /// The metadata blocks, for a `dfLa` box.
    Flac(&'a [u8]),
    /// The 24-byte magic cookie, for an `alac` box.
    Alac(&'a [u8]),
}

fn mp4_box(kind: &[u8; 4], parts: &[&[u8]]) -> Vec<u8> {
    let len = 8 + parts.iter().map(|p| p.len()).sum::<usize>();
    let mut b = (len as u32).to_be_bytes().to_vec();
    b.extend_from_slice(kind);
    for p in parts {
        b.extend_from_slice(p);
    }
    b
}

/// An audio-only MP4 of one track and one chunk: `ftyp`, `moov`, `mdat`.
/// The sample rate is the media timescale (and, above 65535 Hz, 0 in the
/// sample entry's 16.16 field, which cannot hold it).
fn m4a(entry: Entry<'_>, rate: u32, channels: u8, bits: u8, frames: &[(Vec<u8>, u32)]) -> Vec<u8> {
    let u16b = |v: u16| v.to_be_bytes();
    let u32b = |v: u32| v.to_be_bytes();
    let total: u32 = frames.iter().map(|(_, n)| n).sum();
    let matrix: Vec<u8> = [0x0001_0000u32, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000].iter().flat_map(|v| v.to_be_bytes()).collect();

    let (kind, config) = match entry {
        Entry::Flac(blocks) => (b"fLaC", mp4_box(b"dfLa", &[&[0; 4], blocks])),
        Entry::Alac(cookie) => (b"alac", mp4_box(b"alac", &[&[0; 4], cookie])),
    };
    let sample_entry = mp4_box(
        kind,
        &[
            &[0; 6],
            &u16b(1),
            &[0; 8],
            &u16b(u16::from(channels)),
            &u16b(u16::from(bits)),
            &[0; 4],
            &u32b(if rate <= 0xFFFF { rate << 16 } else { 0 }),
            &config,
        ],
    );
    let stsd = mp4_box(b"stsd", &[&[0; 4], &u32b(1), &sample_entry]);
    let mut stts_runs: Vec<(u32, u32)> = Vec::new();
    for &(_, n) in frames {
        match stts_runs.last_mut() {
            Some((count, d)) if *d == n => *count += 1,
            _ => stts_runs.push((1, n)),
        }
    }
    let stts_body: Vec<u8> = stts_runs.iter().flat_map(|&(c, d)| [c.to_be_bytes(), d.to_be_bytes()].concat()).collect();
    let stts = mp4_box(b"stts", &[&[0; 4], &u32b(stts_runs.len() as u32), &stts_body]);
    let stsc = mp4_box(b"stsc", &[&[0; 4], &u32b(1), &u32b(1), &u32b(frames.len() as u32), &u32b(1)]);
    let sizes: Vec<u8> = frames.iter().flat_map(|(f, _)| (f.len() as u32).to_be_bytes()).collect();
    let stsz = mp4_box(b"stsz", &[&[0; 4], &u32b(0), &u32b(frames.len() as u32), &sizes]);
    let ftyp = mp4_box(b"ftyp", &[b"M4A ", &[0; 4], b"M4A mp42isom"]);

    let moov = |mdat_at: u32| -> Vec<u8> {
        let stco = mp4_box(b"stco", &[&[0; 4], &u32b(1), &u32b(mdat_at + 8)]);
        let stbl = mp4_box(b"stbl", &[&stsd, &stts, &stsc, &stsz, &stco]);
        let dinf = mp4_box(b"dinf", &[&mp4_box(b"dref", &[&[0; 4], &u32b(1), &mp4_box(b"url ", &[&[0, 0, 0, 1]])])]);
        let minf = mp4_box(b"minf", &[&mp4_box(b"smhd", &[&[0; 8]]), &dinf, &stbl]);
        let hdlr = mp4_box(b"hdlr", &[&[0; 8], b"soun", &[0; 12], b"\0"]);
        let mdhd = mp4_box(b"mdhd", &[&[0; 12], &u32b(rate), &u32b(total), &u16b(0x55C4), &[0; 2]]);
        let mdia = mp4_box(b"mdia", &[&mdhd, &hdlr, &minf]);
        let tkhd = mp4_box(
            b"tkhd",
            &[&[0, 0, 0, 3], &[0; 8], &u32b(1), &[0; 4], &u32b(total), &[0; 12], &u16b(0x0100), &[0; 2], &matrix, &[0; 8]],
        );
        let trak = mp4_box(b"trak", &[&tkhd, &mdia]);
        let mvhd = mp4_box(
            b"mvhd",
            &[&[0; 12], &u32b(rate), &u32b(total), &u32b(0x0001_0000), &u16b(0x0100), &[0; 10], &matrix, &[0; 24], &u32b(2)],
        );
        mp4_box(b"moov", &[&mvhd, &trak])
    };
    // The chunk offset's value does not change the moov's size.
    let moov_len = moov(0).len();
    let mut out = ftyp.clone();
    out.extend_from_slice(&moov((ftyp.len() + moov_len) as u32));
    let payload: Vec<&[u8]> = frames.iter().map(|(f, _)| f.as_slice()).collect();
    out.extend_from_slice(&mp4_box(b"mdat", &payload));
    out
}

/// An EBML element ID at `at` (its length marker kept): (id, length).
fn ebml_id(data: &[u8], at: usize) -> (u32, usize) {
    let len = data[at].leading_zeros() as usize + 1;
    (data[at..at + len].iter().fold(0u32, |v, &b| (v << 8) | u32::from(b)), len)
}

/// An EBML variable-length integer at `at` (its length marker removed):
/// (value, length); a size of all ones (unknown) comes back as `None`.
fn ebml_vint(data: &[u8], at: usize) -> (Option<u64>, usize) {
    let len = data[at].leading_zeros() as usize + 1;
    let first = u64::from(data[at]) & (0xFF >> len);
    let v = data[at + 1..at + len].iter().fold(first, |v, &b| (v << 8) | u64::from(b));
    let unknown = v == (1u64 << (7 * len)) - 1;
    (if unknown { None } else { Some(v) }, len)
}

/// A Matroska file's one audio track: CodecID, CodecPrivate, and the frames
/// of its SimpleBlocks and Blocks (unlaced, as ffmpeg writes audio).
fn read_matroska(data: &[u8]) -> Track {
    const MASTERS: [u32; 5] = [0x1853_8067, 0x1654_AE6B, 0xAE, 0x1F43_B675, 0xA0];
    let (mut codec, mut config, mut packets) = (None, Vec::new(), Vec::new());
    let mut at = 0;
    while at < data.len() {
        let (id, n) = ebml_id(data, at);
        at += n;
        let (size, n) = ebml_vint(data, at);
        at += n;
        if MASTERS.contains(&id) {
            continue; // descend
        }
        let size = size.expect("an unknown size on an element that is not a master") as usize;
        let body = &data[at..at + size];
        match id {
            0x86 => {
                codec = Some(match body {
                    b"A_FLAC" => "flac",
                    b"A_ALAC" => "alac",
                    other => panic!("CodecID {}", String::from_utf8_lossy(other)),
                })
            }
            0x63A2 => config = body.to_vec(),
            0xA3 | 0xA1 => {
                let (_, track_len) = ebml_vint(body, 0);
                let flags = body[track_len + 2];
                assert_eq!(flags & 0x06, 0, "laced block");
                packets.push(body[track_len + 3..].to_vec());
            }
            _ => {}
        }
        at += size;
    }
    Track { codec: codec.expect("a CodecID"), config, packets }
}
