// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
import { describe, expect, it } from "vitest";
import {
  appComponent,
  bundleIdProblem,
  isPlaceholder,
  prefixProblem,
  replaceBundleId,
  suggestedBundleId,
} from "../../src/setup/bundleId";

describe("bundle identifiers", () => {
  it("knows the placeholders Apple never registers", () => {
    for (const id of ["com.example.test", "COM.EXAMPLE.App", "org.example.x", "net.example"]) {
      expect(isPlaceholder(id), id).toBe(true);
    }
    for (const id of ["com.examples.test", "com.synthetic.example", "io.github.someone.app"]) {
      expect(isPlaceholder(id), id).toBe(false);
    }
  });

  it("accepts a reverse-DNS organization identifier and nothing else", () => {
    expect(prefixProblem("com.synthetic")).toBeUndefined();
    expect(prefixProblem(" io.github.some-one ")).toBeUndefined();
    for (const bad of ["", "synthetic", "com.", ".com.x", "com.some one", "1com.x"]) {
      expect(prefixProblem(bad), bad).toMatch(/reverse-DNS/);
    }
    expect(prefixProblem("com.example")).toMatch(/placeholder/);
  });

  it("checks bundle IDs the same way", () => {
    expect(bundleIdProblem("com.synthetic.my-app2")).toBeUndefined();
    expect(bundleIdProblem("com.synthetic.my app")).toMatch(/letters, digits/);
    expect(bundleIdProblem("test")).toMatch(/letters, digits/);
    expect(bundleIdProblem("com.example.test")).toMatch(/placeholder/);
  });

  it("suggests the app's name under the prefix", () => {
    expect(appComponent("My Great App")).toBe("mygreatapp");
    expect(appComponent("Café-Bar 2")).toBe("cafe-bar2");
    expect(appComponent("日本")).toBe("app");
    expect(suggestedBundleId(" com.synthetic ", "Test")).toBe("com.synthetic.test");
  });

  it("rewrites only the matching bundle-id line of russet.toml", () => {
    const toml = '[app]\nbundle-id = "com.example.test"\n# bundle-id = "com.example.test" in a comment stays\n';
    expect(replaceBundleId(toml, "com.example.test", "com.synthetic.test")).toBe(
      '[app]\nbundle-id = "com.synthetic.test"\n# bundle-id = "com.example.test" in a comment stays\n',
    );
    expect(replaceBundleId(toml, "com.examplextest", "x.y")).toBeUndefined();
    expect(replaceBundleId('  bundle-id="a.b"\n', "a.b", "c.d")).toBe('  bundle-id="c.d"\n');
  });
});
