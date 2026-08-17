//! Measure what a streaming buffer read costs, against the real asset store.
//!
//! M156's entry records that its brief had no number and that this mattered —
//! the "11 MB" it was handed was `music.end`'s *ogg* size, and `music.end` is
//! streamed, so its inline cost is an open plus four one-second chunks rather
//! than 11 MB of decode. This is that measurement for the half M156 left.
//!
//! **It takes file PATHS, not asset keys**, because `rewo-audio` deliberately
//! does not depend on `rewo-data` (see this crate's `Cargo.toml`): the store
//! lookup lives on the caller's side of `PcmSource`.
//!
//! ```text
//! python - <<'EOF'
//! import json, os
//! root = os.path.join(os.environ["APPDATA"], "EwoClient", "shared", "assets")
//! aid = json.load(open(os.path.join(os.environ["APPDATA"], "EwoClient", "shared",
//!                                   "versions", "26.2", "26.2.json")))["assetIndex"]["id"]
//! idx = json.load(open(os.path.join(root, "indexes", f"{aid}.json")))["objects"]
//! h = idx["minecraft/sounds.json"]["hash"]
//! sounds = json.load(open(os.path.join(root, "objects", h[:2], h)))
//! names = {v["name"] for b in sounds.values() for v in b.get("sounds", [])
//!          if isinstance(v, dict) and v.get("stream") and v.get("type", "sound") == "sound"}
//! for n in sorted(names):
//!     e = idx.get(f"minecraft/sounds/{n}.ogg")
//!     if e:
//!         print(f"{n}\t{os.path.join(root, 'objects', e['hash'][:2], e['hash'])}")
//! EOF
//! ```
//!
//! Run: `cargo run --release -p rewo-audio --example measure_stream -- <tsv>`
//!
//! **Release, and run it twice.** The first pass measures a COLD page cache and
//! reports the open four times slower than the second — 9.4 ms against 2.3 ms.
//! Both are real (every music track is a first touch), and reporting either
//! alone is the detector error this project keeps re-finding.

use rewo_audio::buffers::{
    calculate_buffer_size, PcmSource, BUFFER_DURATION_SECONDS, QUEUED_BUFFER_COUNT,
};

fn main() {
    let tsv = match std::env::args().nth(1) {
        Some(p) => p,
        None => {
            eprintln!("usage: measure_stream <tsv of name\\tpath>");
            std::process::exit(2);
        }
    };
    let body = std::fs::read_to_string(&tsv).expect("read tsv");
    let rows: Vec<(String, String)> = body
        .lines()
        .filter_map(|l| {
            let mut it = l.split('\t');
            Some((it.next()?.to_string(), it.next()?.to_string()))
        })
        .collect();
    println!("{} streamed variants to measure", rows.len());

    let mut out: Vec<(String, f64, f64, f64, u16, u32)> = Vec::new();
    for (name, path) in &rows {
        let p = path.clone();
        let mut src = rewo_audio::decode::BytesSource(move |_k: &str| {
            std::fs::read(&p).map_err(|e| e.to_string())
        });

        let t0 = std::time::Instant::now();
        let mut st = match src.open_stream(name, false) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("  skip {name}: {e}");
                continue;
            }
        };
        let open_ms = t0.elapsed().as_secs_f64() * 1000.0;

        let (ch, rate) = st.format();
        let per = calculate_buffer_size(ch, rate, BUFFER_DURATION_SECONDS) / 2;

        // The four `attachBufferStream` pumps: what STARTING a stream costs.
        let t1 = std::time::Instant::now();
        for _ in 0..QUEUED_BUFFER_COUNT {
            match st.read(per) {
                Ok(c) if c.is_empty() => break,
                Ok(_) => {}
                Err(_) => break,
            }
        }
        let prime_ms = t1.elapsed().as_secs_f64() * 1000.0;

        // A steady-state refill: one buffer, well past the header.
        let t2 = std::time::Instant::now();
        let mut n = 0u32;
        for _ in 0..8 {
            match st.read(per) {
                Ok(c) if c.is_empty() => break,
                Ok(_) => n += 1,
                Err(_) => break,
            }
        }
        let one_ms = if n > 0 {
            t2.elapsed().as_secs_f64() * 1000.0 / n as f64
        } else {
            0.0
        };
        out.push((name.clone(), open_ms, prime_ms, one_ms, ch, rate));
    }

    out.sort_by(|a, b| b.3.total_cmp(&a.3));
    println!(
        "\n{:<44} {:>8} {:>9} {:>10} {:>4} {:>7}",
        "key", "open ms", "prime ms", "1 buf ms", "ch", "rate"
    );
    for (k, o, p, one, c, r) in out.iter().take(12) {
        println!("{k:<44} {o:>8.2} {p:>9.2} {one:>10.2} {c:>4} {r:>7}");
    }
    let n = out.len() as f64;
    println!(
        "\nmean over {}: open {:.2} ms, prime(4) {:.2} ms, ONE BUFFER {:.2} ms",
        out.len(),
        out.iter().map(|r| r.1).sum::<f64>() / n,
        out.iter().map(|r| r.2).sum::<f64>() / n,
        out.iter().map(|r| r.3).sum::<f64>() / n,
    );
    let worst_one = out.first().map(|r| r.3).unwrap_or(0.0);
    let worst_prime = out.iter().map(|r| r.2).fold(0.0f64, f64::max);
    println!(
        "worst single buffer {worst_one:.2} ms, worst prime {worst_prime:.2} ms, \
         against a 50 ms client tick."
    );
}
