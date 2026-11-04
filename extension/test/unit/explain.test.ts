// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
import { describe, it, expect } from "vitest";
import { explain } from "../../src/run/explain";

describe("compiler errors in plain words", () => {
  it("names SwiftUI's own name for another toolkit's", () => {
    // The owner's work PC, 2026-11-04.
    expect(explain("cannot find 'VBox' in scope")).toBe("SwiftUI has no 'VBox': it is called VStack.");
    expect(explain("cannot find 'HBox' in scope")).toBe("SwiftUI has no 'HBox': it is called HStack.");
  });

  it("explains the common errors with their names filled in", () => {
    expect(explain("cannot find 'ContentView' in scope")).toMatch(/^Nothing named 'ContentView' is visible here\./);
    expect(explain("cannot find type 'Bubble' in scope")).toMatch(/^No type named 'Bubble'/);
    expect(explain("value of type 'Game' has no member 'scroe'")).toBe(
      "A value of type Game has no property or method called 'scroe'. Check the name, or the value is not of the type you think.",
    );
    expect(explain("cannot convert value of type 'Int' to expected argument type 'String'")).toBe(
      "A value of type Int is used where String is needed. Convert it, or pass a value of the right type.",
    );
    expect(explain("'symbolEffect' is only available in iOS 18.0 or newer")).toMatch(/needs iOS 18\.0 or newer/);
    expect(explain("missing return in closure expected to return 'Int'")).toMatch(/without returning a value of type Int/);
  });

  it("says nothing about an error it does not know", () => {
    expect(explain("something unusual happened")).toBeUndefined();
  });
});
