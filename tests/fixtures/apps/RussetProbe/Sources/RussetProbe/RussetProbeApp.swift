// SPDX-License-Identifier: 0BSD
// SPDX-FileCopyrightText: 2026 Russet contributors

import Foundation
import SwiftUI

/// The app entry point. Besides the UI, RussetProbe writes a heartbeat to stdout every 100 ms,
/// `PROBE seq=<n> t=<ms>` (ms since launch), which the lab scripts count (PLAN §4.10), and crashes on purpose
/// when launched with `-RussetProbeCrash 1` (crash collection and symbolication checks).
@main
struct RussetProbeApp: App {
    init() {
        Heartbeat.shared.start(crash: UserDefaults.standard.bool(forKey: "RussetProbeCrash"))
    }

    var body: some Scene {
        WindowGroup {
            ContentView()
        }
    }
}

/// The stdout heartbeat. Runs on its own queue, so it keeps going while the UI is busy.
final class Heartbeat: @unchecked Sendable {
    static let shared = Heartbeat()

    /// Heartbeats written before the forced crash, so the crash happens after a launch the scripts can see.
    static let crashAfter = 10

    private let queue = DispatchQueue(label: "dev.russet.test.probe.heartbeat", qos: .utility)
    private var timer: DispatchSourceTimer?
    private var seq = 0
    private let start = DispatchTime.now().uptimeNanoseconds

    func start(crash: Bool) {
        // Line-buffered stdout: a pipe or socket would otherwise hold the lines back.
        setvbuf(stdout, nil, _IOLBF, 0)
        let timer = DispatchSource.makeTimerSource(queue: queue)
        timer.schedule(deadline: .now(), repeating: .milliseconds(100))
        timer.setEventHandler { [weak self] in
            self?.beat(crash: crash)
        }
        self.timer = timer
        timer.resume()
    }

    private func beat(crash: Bool) {
        let ms = (DispatchTime.now().uptimeNanoseconds - start) / 1_000_000
        print("PROBE seq=\(seq) t=\(ms)")
        fflush(stdout)
        seq += 1
        if crash && seq > Heartbeat.crashAfter {
            fatalError("RussetProbe: forced crash (-RussetProbeCrash 1)")
        }
    }
}
