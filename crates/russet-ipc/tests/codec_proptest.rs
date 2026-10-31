// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Framing with arbitrary chunking (PLAN W2 tests; core §19 "Property"): any sequence of frames, with any
//! header spelling and extra headers, split into any chunks, decodes to the same bodies in order.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use bytes::{Bytes, BytesMut};
use proptest::prelude::*;
use russet_ipc::codec::LspCodec;
use tokio_util::codec::{Decoder, Encoder};

/// A frame as a sender might write it: the body, the header name's spelling, and an optional Content-Type.
#[derive(Debug, Clone)]
struct Frame {
    body: Vec<u8>,
    header: String,
    content_type: bool,
    type_first: bool,
}

fn frame() -> impl Strategy<Value = Frame> {
    (
        proptest::collection::vec(any::<u8>(), 0..300),
        prop_oneof![
            Just("Content-Length".to_owned()),
            Just("content-length".to_owned()),
            Just("CONTENT-LENGTH".to_owned()),
            Just("Content-length".to_owned()),
        ],
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(|(body, header, content_type, type_first)| Frame { body, header, content_type, type_first })
}

fn encode(f: &Frame) -> Vec<u8> {
    let mut out = Vec::new();
    let length = format!("{}: {}\r\n", f.header, f.body.len());
    let ctype = "Content-Type: application/vscode-jsonrpc; charset=utf-8\r\n";
    if f.content_type && f.type_first {
        out.extend_from_slice(ctype.as_bytes());
    }
    out.extend_from_slice(length.as_bytes());
    if f.content_type && !f.type_first {
        out.extend_from_slice(ctype.as_bytes());
    }
    out.extend_from_slice(b"\r\n");
    out.extend_from_slice(&f.body);
    out
}

proptest! {
    #[test]
    fn frames_survive_arbitrary_chunking(
        frames in proptest::collection::vec(frame(), 1..12),
        cuts in proptest::collection::vec(1usize..64, 1..200),
    ) {
        let stream: Vec<u8> = frames.iter().flat_map(encode).collect();
        let mut codec = LspCodec::default();
        let mut buf = BytesMut::new();
        let mut decoded = Vec::new();
        let mut offset = 0;
        let mut cut = cuts.iter().cycle();
        while offset < stream.len() {
            let n = (*cut.next().unwrap()).min(stream.len() - offset);
            buf.extend_from_slice(&stream[offset..offset + n]);
            offset += n;
            while let Some(body) = codec.decode(&mut buf).expect("valid frames decode") {
                decoded.push(body.to_vec());
            }
        }
        let expected: Vec<Vec<u8>> = frames.iter().map(|f| f.body.clone()).collect();
        prop_assert_eq!(decoded, expected);
        prop_assert!(buf.is_empty());
    }

    #[test]
    fn what_the_encoder_writes_the_decoder_reads(
        bodies in proptest::collection::vec(proptest::collection::vec(any::<u8>(), 0..500), 1..8),
        chunk in 1usize..97,
    ) {
        let mut codec = LspCodec::default();
        let mut wire = BytesMut::new();
        for b in &bodies {
            codec.encode(Bytes::from(b.clone()), &mut wire).expect("encodes");
        }
        let mut buf = BytesMut::new();
        let mut decoded = Vec::new();
        for piece in wire.chunks(chunk) {
            buf.extend_from_slice(piece);
            while let Some(body) = codec.decode(&mut buf).expect("decodes") {
                decoded.push(body.to_vec());
            }
        }
        prop_assert_eq!(decoded, bodies);
    }
}
