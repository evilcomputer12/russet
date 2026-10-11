// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! H6 (RD §23.2, V5): LLDB on the `remote-ios` platform connects through the broker to the fake iOS debugserver
//! and binds the local arm64 executable to the "device" image by UUID, with neither `SetPlatformFileSpec` nor
//! Python, using exactly the generated attach commands.

// Unix sockets, /proc and lldb-server: these harnesses run on Linux (and macOS) hosts.
#![cfg(unix)]
#![allow(clippy::unwrap_used)]

mod common;

use std::time::Duration;

use russet_debug::BoxStream;
use russet_debug::broker::{Broker, BrokerOptions, BrokerState, EndpointSpec};
use russet_debug::lldb::{self, AttachPlan};
use russet_testkit::debug::{FakeDebugserver, FakeImage, FakeSpec, LldbTools, macho};

#[tokio::test(flavor = "multi_thread")]
async fn lldb_binds_the_local_executable_by_uuid_through_the_broker() {
    let Some(tools) = LldbTools::for_test("H6", true) else { return };
    let dir = common::scratch("h6");
    let exe = dir.join("RussetProbe-local");
    let uuid = [0x4C, 0x4C, 0x44, 0xA9, 0x55, 0x55, 0x31, 0x44, 0xA1, 0xB2, 0x60, 0xBF, 0xA6, 0x6E, 0x44, 0x25];
    std::fs::write(&exe, macho::synthetic_ios_executable(uuid)).unwrap();
    let device_path = "/private/var/containers/Bundle/Application/1C2/RussetProbe.app/RussetProbe";
    let fake = FakeDebugserver::start(FakeSpec {
        pid: 932,
        image: Some(FakeImage { device_path: device_path.into(), macho: std::fs::read(&exe).unwrap() }),
        ..FakeSpec::default()
    })
    .unwrap();
    let upstream: BoxStream = Box::new(tokio::net::TcpStream::connect(fake.addr()).await.unwrap());
    let mut broker = Broker::connect(upstream, BrokerOptions::default()).await.unwrap();
    broker.attach(932).await.unwrap();
    let handle = broker.serve(EndpointSpec::Unix { path: dir.join("s/gdb.sock") }).await.unwrap();

    // The device plan, without search paths: the UUID match alone binds the file (V5; device check D4).
    let plan = AttachPlan::ios(exe.clone(), handle.lldb_url());
    let mut commands = lldb::init_commands(&lldb::InitSettings::default());
    commands.extend(lldb::attach_commands(&plan));
    commands.extend(["image list".to_owned(), "process kill".to_owned()]);
    assert!(!lldb::uses_python(&commands));
    let script = dir.join("h6.lldb");
    std::fs::write(&script, lldb::script_text(&commands)).unwrap();
    let run = common::run_lldb(&tools, &lldb::batch_args(&script), Duration::from_secs(60)).await;
    let out = run.stdout;
    assert!(out.contains("Process 932 stopped"), "{out}\n{}", run.stderr);
    let image_line = out
        .lines()
        .find(|l| l.contains("4C4C44A9-5555-3144-A1B2-60BFA66E4425"))
        .unwrap_or_else(|| panic!("no image with the UUID:\n{out}"));
    assert!(image_line.contains("0x0000000100000000"), "{image_line}");
    assert!(image_line.contains("RussetProbe-local"), "the local file, not a memory image: {image_line}");
    assert!(fake.report().killed);
    assert!(matches!(handle.wait_for(|_| false).await, BrokerState::Exited { .. }));
    let report = fake.report();
    assert!(report.saw("jGetLoadedDynamicLibrariesInfos:{"), "LLDB asked dyld for the images");
    assert!(!report.saw("vAttach;3a4") || report.attach_count == 1, "only the broker attached");
}
