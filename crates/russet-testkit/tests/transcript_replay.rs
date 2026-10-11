// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Transcript replay (CORE-15) over the russet/1 framing, against the testkit's fake daemon.

use std::collections::BTreeMap;
use std::time::Duration;

use russet_testkit::transcript::{ErrorKind, matches};
use russet_testkit::{Transcript, fake_daemon, lsp};
use serde_json::{Value, json};
use tokio::io::BufReader;

/// A fake daemon on the far end of an in-memory duplex.
fn fake() -> tokio::io::DuplexStream {
    let (client, server) = tokio::io::duplex(64 * 1024);
    fake_daemon::serve(server);
    client
}

fn init() -> Value {
    json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocol": {"major": 1, "minor": 0}}})
}

#[tokio::test]
async fn the_committed_transcript_replays_against_the_fake_daemon() {
    let path = russet_testkit::testkit_dir().join("transcripts/fake-daemon.transcript.jsonl");
    let t = Transcript::from_file(&path).expect("transcript");
    assert_eq!(t.name.as_deref(), Some("fake-daemon"));
    let replay = t.replay(fake()).await.unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(replay.captures["conn"], json!("c-1"));
    assert_eq!(replay.captures["confirm"], json!("s:1"));
    assert!(replay.skipped.iter().all(|m| m["method"] == json!("$/progress")), "only progress is skipped");
}

#[tokio::test]
async fn a_wrong_answer_names_the_path() {
    let t = Transcript::default()
        .send(init())
        .expect(json!({"id": 1, "result": {"protocol": {"major": 2}}}));
    let e = t.replay(fake()).await.expect_err("major 1 is not 2");
    assert_eq!(e.line, 2);
    match &e.kind {
        ErrorKind::Mismatch { path, .. } => assert_eq!(path, "$.result.protocol.major"),
        other => panic!("{other:?}"),
    }
    assert!(e.to_string().contains("received:"), "{e}");
}

#[tokio::test]
async fn silence_times_out_and_a_closed_connection_is_reported() {
    let mut t = Transcript::default().send(init()).expect(json!({"id": 1})).expect(json!({"id": 99}));
    t.timeout = Some(Duration::from_millis(100));
    let e = t.replay(fake()).await.expect_err("nothing answers id 99");
    assert!(matches!(e.kind, ErrorKind::Timeout { .. }), "{e}");
    let (client, server) = tokio::io::duplex(1024);
    drop(server);
    let e = Transcript::default().expect(json!({"id": 1})).replay(client).await.expect_err("closed");
    assert!(matches!(e.kind, ErrorKind::Closed { .. } | ErrorKind::Io(_)), "{e}");
}

#[test]
fn transcripts_round_trip_and_reject_bad_lines() {
    let t = Transcript::default().ignoring(&["daemon.logBatch"]).send(init()).expect(json!({"id": 1, "result": "$any"}));
    let again = Transcript::parse(&t.to_jsonl()).expect("parse");
    assert_eq!(again.ignore, t.ignore);
    assert_eq!(again.entries.len(), 2);
    for (bad, why) in [
        ("{\"send\": {}, \"expect\": {}}", "exactly one"),
        ("[1, 2]", "JSON object"),
        ("{\"transcript\": 2}", "unsupported transcript version"),
        ("not json", "expected"),
    ] {
        let e = Transcript::parse(bad).expect_err(why);
        assert!(e.to_string().contains(why), "{why}: {e}");
    }
}

#[test]
fn patterns() {
    let mut caps = BTreeMap::new();
    let actual = json!({"a": 1, "b": [1, 2, 3], "c": "c-7", "d": null});
    assert!(matches(&json!({"a": 1.0}), &actual, &mut caps, "$").is_ok(), "numbers compare by value");
    assert!(matches(&json!({"b": ["$contains", 3, 1]}), &actual, &mut caps, "$").is_ok());
    assert!(matches(&json!({"b": ["$contains", 4]}), &actual, &mut caps, "$").is_err());
    assert!(matches(&json!({"c": "$prefix:c-", "d": "$type:null", "e": "$absent"}), &actual, &mut caps, "$").is_ok());
    assert!(matches(&json!({"a": "$any", "$exact": true}), &actual, &mut caps, "$").is_err());
    assert!(matches(&json!({"c": "$capture:conn"}), &actual, &mut caps, "$").is_ok());
    assert!(matches(&json!({"c": "${conn}"}), &actual, &mut caps, "$").is_ok());
    assert!(matches(&json!({"a": "${conn}"}), &actual, &mut caps, "$").is_err());
    assert_eq!(matches(&json!({"b": [1, 2]}), &actual, &mut caps, "$").map_err(|e| e.0), Err("$.b".to_string()));
}

#[tokio::test]
async fn the_codec_survives_split_frames_and_rejects_huge_ones() {
    let msg = json!({"jsonrpc": "2.0", "id": 7, "result": {"text": "ü€ multibyte"}});
    let bytes = lsp::frame_bytes(&msg);
    let (mut w, r) = tokio::io::duplex(8);
    let writer = tokio::spawn(async move {
        for chunk in bytes.chunks(3) {
            tokio::io::AsyncWriteExt::write_all(&mut w, chunk).await.expect("write");
        }
    });
    let mut reader = BufReader::new(r);
    assert_eq!(lsp::read_message(&mut reader).await.expect("read"), Some(msg));
    writer.await.expect("writer");
    assert_eq!(lsp::read_message(&mut reader).await.expect("eof"), None);

    let huge = format!("content-length: {}\r\ncontent-type: x\r\n\r\n", lsp::MAX_FRAME + 1);
    let mut r = BufReader::new(huge.as_bytes());
    let e = lsp::read_message(&mut r).await.expect_err("too large");
    assert!(e.to_string().contains("16 MiB"), "{e}");
}
