// SPDX-License-Identifier: 0BSD
// SPDX-FileCopyrightText: 2026 Russet contributors

import SwiftUI

/// The debugger target. Scripts find the `RUSSET_PROBE_BREAK` marker with grep and break on the line after it,
/// inside `ContentView.body.getter`, then read `frame variable probeValue` (a plain local; `@State` properties
/// are only reachable as `self._counter`). Keep the two lines below together and in this order (PLAN §4.5).
struct ContentView: View {
    @State var counter = 42
    var body: some View {
        let probeValue = counter  // RUSSET_PROBE_BREAK
        Text("counter \(probeValue)")  // the breakpoint line: the line after the marker
    }
}
