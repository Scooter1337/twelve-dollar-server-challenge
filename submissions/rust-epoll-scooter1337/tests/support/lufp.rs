use super::*;
use crate::serde_oracle;

fn same_body(raw: &[u8]) {
    assert_eq!(
        post_body(raw).map_err(|_| ()),
        serde_oracle::body(raw).map_err(|_| ()),
        "{raw:?}"
    );
}

#[test]
fn body_matches_original_for_unicode_duplicates_and_errors() {
    for raw in [
        br#"{"body":"hello"}"#.as_slice(),
        br#"{"body":"first","body":"last"}"#,
        br#"{"body":"first","body":null}"#,
        br#"{"body":{"body":"nested"}}"#,
        br#"{"b\u006fdy":"a\"b\\c\n\uD83D\uDE80"}"#,
        br#"{"before":[null,true,1e-6,{"a":false}],"body":"yes","after":{}}"#,
        br#"{"body":"yes","ignored":"\ud800"}"#,
        br#"{"body":"\u0000abc"}"#,
        br#"{"body":"yes","ignored":1e999}"#,
        br#"{"body":"yes","ignored":1e-999}"#,
        br#"{"body":"yes","ignored":01}"#,
        br#"{"body":"yes","ignored":[1,]}"#,
        br#"{"body":"yes"}x"#,
        br#"{"body":"\udc00"}"#,
        br#"{"body":"\ud800\u0000"}"#,
        b"{\"body\":\"\xff\"}",
        b"{\"body\":\"\x01\"}",
        b"null",
        b"[]",
        b"true",
        b"false",
        b"1.23e4",
        br#""text""#,
        b"",
        b"{",
        b"[1,]",
    ] {
        same_body(raw);
    }
    let long = "😀".repeat(500);
    for text in ["café 🚀", "\u{feff} \u{a0}", long.as_str()] {
        same_body(
            serde_json::to_string(&serde_json::json!({"body": text}))
                .unwrap()
                .as_bytes(),
        );
    }
    for depth in [125, 126, 127, 128, 129] {
        same_body(format!("{}0{}", "[".repeat(depth), "]".repeat(depth)).as_bytes());
        same_body(
            format!(
                "{{\"body\":\"ok\",\"x\":{}0{}}}",
                "[".repeat(depth),
                "]".repeat(depth)
            )
            .as_bytes(),
        );
    }
    assert!(matches!(
        post_body(br#"{"body":"plain"}"#).unwrap(),
        Some(Cow::Borrowed("plain"))
    ));
}

#[test]
fn claims_match_original_and_require_object_roots() {
    for raw in [
        br#"{"sub":"1","username":"name","iat":12,"exp":4102444800}"#.as_slice(),
        br#"{"username":"na\u006de \uD83D\uDE80","exp":4.1024448e9,"s\u0075b":"1","nbf":null}"#,
        br#"{"sub":"1","username":"name","exp":4102444800,"nbf":1.5,"x":{"a":[true,null]}}"#,
        br#"{"sub":2,"username":"name","exp":4102444800}"#,
        br#"{"sub":"1","username":null,"exp":4102444800}"#,
        br#"{"sub":"1","username":"name","exp":null}"#,
        br#"{"sub":"1","username":"name","exp":1e999}"#,
        br#"{"sub":"1","sub":"2","username":"name","exp":4102444800}"#,
        br#"{"exp":null,"exp":4102444800}"#,
        br#"{"sub":"1","username":"name","exp":4102444800}garbage"#,
    ] {
        let actual = claims(raw).map_err(|_| ()).and_then(|c| {
            Ok((
                c.sub.ok_or(())?,
                c.username.ok_or(())?,
                c.exp.ok_or(())?,
                c.nbf,
            ))
        });
        assert_eq!(actual, serde_oracle::identity(raw), "{raw:?}");
    }
    for raw in [
        br#"["1","name",4102444800,null]"#.as_slice(),
        b"null",
        b"true",
    ] {
        assert!(claims(raw).is_err());
    }
    for raw in [
        br#"{"alg":"HS256"}"#.as_slice(),
        br#"{"typ":"JWT","a\u006cg":"HS256","x":[true]}"#,
    ] {
        assert!(header(raw).is_ok());
    }
    for raw in [
        br#"["HS256"]"#.as_slice(),
        br#"{"alg":"none"}"#,
        br#"{"alg":"HS256","alg":"HS256"}"#,
        br#"{"alg":"HS256","x":[1,]}"#,
        br#"{"alg":"HS256"}x"#,
    ] {
        assert!(header(raw).is_err());
    }
}

#[test]
fn mutated_documents_match_the_original_body_parser() {
    let seeds: &[&[u8]] = &[
        br#"{"body":"plain"}"#,
        br#"{"before":[true,null,1.234e-7,{"key":"caf\u00e9"}],"body":"x\n\uD83D\uDE80","after":false}"#,
        br#"{"body":"first","b\u006fdy":"second","ignored":{"nested":[1,2,3]}}"#,
    ];
    let mut rng = 0x9e3779b97f4a7c15u64;
    for _ in 0..60_000 {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        let mut raw = seeds[(rng as usize) % seeds.len()].to_vec();
        let position = (rng >> 16) as usize % raw.len();
        match (rng >> 32) % 3 {
            0 => raw[position] = (rng >> 40) as u8,
            1 => {
                raw.remove(position);
            }
            _ => raw.insert(position, (rng >> 40) as u8),
        }
        same_body(&raw);
    }
}

#[test]
#[ignore = "diagnostic parser benchmark; run on an otherwise idle Ubuntu CPU"]
fn parser_benchmark() {
    use std::{hint::black_box, time::Instant};
    let h = br#"{"alg":"HS256","typ":"JWT"}"#;
    let c = br#"{"sub":"12345","username":"golden_ember_12345","iat":1792100000,"exp":4102444800}"#;
    let b = br#"{"body":"allocation comparison post"}"#;
    for trial in 0..12 {
        for candidate in if trial % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        } {
            let start = Instant::now();
            for _ in 0..500_000 {
                if candidate {
                    black_box(header(black_box(h)).unwrap());
                    black_box(claims(black_box(c)).unwrap());
                    black_box(post_body(black_box(b)).unwrap());
                } else {
                    black_box(serde_oracle::header(black_box(h)).unwrap());
                    black_box(serde_oracle::identity(black_box(c)).unwrap());
                    black_box(serde_oracle::body(black_box(b)).unwrap());
                }
            }
            eprintln!(
                "PARSER trial={trial} candidate={candidate} ns={:.3}",
                start.elapsed().as_nanos() as f64 / 500_000.0
            );
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn string_scanner_stays_inside_guarded_input() {
    // End each input immediately before an unreadable page. Varying lengths
    // exercises every SIMD alignment and truncated document tail.
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as usize;
    assert!(page >= 4096);
    let raw = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            page * 2,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    assert_ne!(raw, libc::MAP_FAILED);
    struct Mapping(*mut libc::c_void, usize);
    impl Drop for Mapping {
        fn drop(&mut self) {
            unsafe {
                libc::munmap(self.0, self.1);
            }
        }
    }
    let _mapping = Mapping(raw, page * 2);
    assert_eq!(
        unsafe { libc::mprotect(raw.cast::<u8>().add(page).cast(), page, libc::PROT_NONE) },
        0
    );
    for length in 0..96 {
        for tail in ["plain", "café 🚀", "quote\" and slash\\"] {
            let body = format!("{}{tail}", "a".repeat(length));
            let document = serde_json::to_vec(&serde_json::json!({"body": body})).unwrap();
            for end in 0..=document.len() {
                let start = unsafe { raw.cast::<u8>().add(page - end) };
                unsafe {
                    std::ptr::copy_nonoverlapping(document.as_ptr(), start, end);
                }
                let input = unsafe { std::slice::from_raw_parts(start, end) };
                same_body(input);
            }
        }
    }
}
