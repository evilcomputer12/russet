// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors
//
// Apple's files in the editor beyond what the manifest's languages and grammars give: a binary
// property list (a built app's Info.plist, a compiled .strings) opens as XML and saves back as
// binary (`russet-plist:`), and a provisioning profile opens as its XML, read-only
// (`russet-profile:`). A binary plist opened as text switches to the XML view by itself.

import * as vscode from "vscode";
import { log } from "../util/log";
import { encodeBinaryPlist, isBinaryPlist, parseBinaryPlist, parseXmlPlist, profileXml, toXmlPlist } from "./plist";

const PLIST = "russet-plist";
const PROFILE = "russet-profile";
const PROFILE_EXT = /\.(mobileprovision|provisionprofile)$/i;

const onDisk = (uri: vscode.Uri): vscode.Uri => uri.with({ scheme: "file" });

/** Files on disk seen through a conversion: XML for the editor, the original format on disk. */
class ConvertingFileSystem implements vscode.FileSystemProvider {
  private readonly changed = new vscode.EventEmitter<vscode.FileChangeEvent[]>();
  readonly onDidChangeFile = this.changed.event;

  constructor(
    private readonly toText: (bytes: Uint8Array) => string,
    private readonly fromText?: (text: string) => Uint8Array,
  ) {}

  watch(): vscode.Disposable {
    // Saves through this view report themselves (writeFile); outside changes show on reopening.
    return new vscode.Disposable(() => undefined);
  }

  async stat(uri: vscode.Uri): Promise<vscode.FileStat> {
    const s = await vscode.workspace.fs.stat(onDisk(uri));
    // The size of the text this view shows, not of the file on disk.
    const size = (await this.readFile(uri)).byteLength;
    return { type: s.type, ctime: s.ctime, mtime: s.mtime, size, ...(this.fromText ? {} : { permissions: vscode.FilePermission.Readonly }) };
  }

  async readFile(uri: vscode.Uri): Promise<Uint8Array> {
    try {
      return new TextEncoder().encode(this.toText(await vscode.workspace.fs.readFile(onDisk(uri))));
    } catch (err) {
      log().warn(`[apple] ${uri.scheme} could not read ${uri.path}: ${(err as Error).message}`);
      throw vscode.FileSystemError.Unavailable(`${uri.fsPath}: ${(err as Error).message}`);
    }
  }

  async writeFile(uri: vscode.Uri, content: Uint8Array): Promise<void> {
    if (!this.fromText) {
      throw vscode.FileSystemError.NoPermissions("Provisioning profiles are signed; Russet opens them read-only.");
    }
    let bytes: Uint8Array;
    try {
      bytes = this.fromText(new TextDecoder().decode(content));
    } catch (err) {
      throw vscode.FileSystemError.Unavailable(`Not saved, the property list does not parse: ${(err as Error).message}`);
    }
    await vscode.workspace.fs.writeFile(onDisk(uri), bytes);
    this.changed.fire([{ type: vscode.FileChangeType.Changed, uri }]);
  }

  readDirectory(): never {
    throw vscode.FileSystemError.NoPermissions();
  }
  createDirectory(): never {
    throw vscode.FileSystemError.NoPermissions();
  }
  delete(): never {
    throw vscode.FileSystemError.NoPermissions();
  }
  rename(): never {
    throw vscode.FileSystemError.NoPermissions();
  }
}

/** The view a file opens in: its profile view, its XML view when binary, or the file itself. */
export async function appleView(file: vscode.Uri): Promise<vscode.Uri> {
  if (PROFILE_EXT.test(file.path)) {
    return file.with({ scheme: PROFILE });
  }
  const head = (await vscode.workspace.fs.readFile(file)).subarray(0, 8);
  return isBinaryPlist(head) ? file.with({ scheme: PLIST }) : file;
}

export function registerAppleFiles(context: vscode.ExtensionContext): void {
  const plist = new ConvertingFileSystem(
    (bytes) => toXmlPlist(parseBinaryPlist(bytes)),
    (text) => encodeBinaryPlist(parseXmlPlist(text)),
  );
  const profile = new ConvertingFileSystem((bytes) => {
    const xml = profileXml(bytes);
    if (!xml) throw new Error("No property list inside this provisioning profile.");
    return xml;
  });
  const switching = new Set<string>();
  context.subscriptions.push(
    vscode.workspace.registerFileSystemProvider(PLIST, plist, { isCaseSensitive: process.platform !== "win32" }),
    vscode.workspace.registerFileSystemProvider(PROFILE, profile, { isCaseSensitive: process.platform !== "win32", isReadonly: true }),
    vscode.commands.registerCommand("russet.apple.openPlist", async (uri?: vscode.Uri) => {
      const file = uri ?? vscode.window.activeTextEditor?.document.uri;
      if (file?.scheme === "file") {
        await vscode.commands.executeCommand("vscode.open", await appleView(file));
      }
    }),
    // A binary plist opened as text shows as noise: switch its tab to the XML view.
    vscode.workspace.onDidOpenTextDocument(async (doc) => {
      if (doc.uri.scheme !== "file" || !doc.getText(new vscode.Range(0, 0, 0, 8)).startsWith("bplist") || switching.has(doc.uri.toString())) {
        return;
      }
      switching.add(doc.uri.toString());
      try {
        await vscode.commands.executeCommand("vscode.open", doc.uri.with({ scheme: PLIST }), { preview: false });
        const tabs = vscode.window.tabGroups.all.flatMap((g) => g.tabs)
          .filter((t) => t.input instanceof vscode.TabInputText && t.input.uri.toString() === doc.uri.toString());
        await vscode.window.tabGroups.close(tabs);
      } finally {
        switching.delete(doc.uri.toString());
      }
    }),
  );
}
