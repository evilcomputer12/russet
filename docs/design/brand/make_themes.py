# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Russet contributors
"""Writes Russet's colour themes into extension/themes and checks that every text colour keeps
WCAG AA contrast (4.5:1) against its background:

- Russet Dark and Russet Light, from the crab apple's palette (russet browns, crab orange, leaf
  green, russeting gold);
- Russet Studio Dark and Light, Xcode-style: the roles of Xcode's Default themes (pink keywords,
  red strings, declarations apart from references, project symbols apart from the SDK's) on
  macOS-like neutral greys with the system blue accent. Colours Xcode draws below AA (dark
  comments, line numbers) are lifted just enough to pass.

    python3 make_themes.py
"""
import json
import pathlib
import sys

HERE = pathlib.Path(__file__).resolve().parent
THEMES = HERE.parents[2] / "extension" / "themes"

PALETTES = {
    "dark": dict(
        bg="#1E1A17", panel="#191613", deep="#14110F", raised="#26211D", line="#3A3029",
        fg="#EADBC8", muted="#A8988A", accent="#E07A3F", accentFg="#1E1A17", select="#4A3426",
        status="#7B3618", statusFg="#FFF3E0", lineHi="#2A241F", find="#6B4A1F",
        comment="#9C8B7C", keyword="#F08A4B", string="#A9C46C", number="#E8B865", function="#F2B880",
        type="#E3A35F", variable="#EADBC8", constant="#E8B865", tag="#F08A4B", attribute="#D9A86A",
        error="#F2705A", warning="#E8B865", info="#8FB8D9",
    ),
    "light": dict(
        bg="#FBF7F2", panel="#F3EDE5", deep="#EBE3D8", raised="#FFFFFF", line="#DCCFBF",
        fg="#3A2618", muted="#6E5A4A", accent="#A9521F", accentFg="#FFFFFF", select="#F0D9C3",
        status="#7B3618", statusFg="#FFF3E0", lineHi="#F3EADF", find="#F2D29A",
        comment="#7A6B5E", keyword="#A8401A", string="#4A7322", number="#8C5A0C", function="#8A3E14",
        type="#7A4A1A", variable="#3A2618", constant="#8C5A0C", tag="#A8401A", attribute="#7A4A1A",
        error="#B3261E", warning="#8C5A0C", info="#2F6690",
    ),
    "studio-dark": dict(
        bg="#1F1F24", panel="#252528", deep="#1C1C1E", raised="#2C2C2E", line="#38383A",
        fg="#DFDFE0", muted="#8E8E93", accent="#3D93F8", accentFg="#000000", select="#515B70",
        status="#2C2C2E", statusFg="#DFDFE0", lineHi="#23252B", find="#6E5A1A",
        comment="#7F8C98", keyword="#FC5FA3", string="#FC6A5D", number="#D0BF69", function="#67B7A4",
        type="#9EF1DD", variable="#DFDFE0", constant="#67B7A4", tag="#FC5FA3", attribute="#BF8555",
        error="#FF6B5D", warning="#FFD60A", info="#64D2FF",
        preprocessor="#FD8F3F", url="#6B92FF", typeDecl="#5DD8FF", otherDecl="#41A1C0",
        sdkType="#D0A8FF", sdkFunction="#B281EB", heading="#92A1B1",
    ),
    "studio-light": dict(
        bg="#FFFFFF", panel="#F5F5F7", deep="#ECECEE", raised="#FFFFFF", line="#D1D1D6",
        fg="#262626", muted="#6E6E73", accent="#0064D1", accentFg="#FFFFFF", select="#B3D7FF",
        status="#F5F5F7", statusFg="#3C3C43", lineHi="#E8F2FF", find="#FFE58A",
        comment="#5D6C79", keyword="#9B2393", string="#C41A16", number="#1C00CF", function="#326D74",
        type="#1C464A", variable="#262626", constant="#326D74", tag="#9B2393", attribute="#815F03",
        error="#C41A16", warning="#8A6100", info="#0064D1",
        preprocessor="#643820", url="#0E0EFF", typeDecl="#0B4F79", otherDecl="#0F68A0",
        sdkType="#3900A0", sdkFunction="#6C36A9", heading="#4A5560",
    ),
}
# Each theme's label in the theme picker, which settings also use as its id.
NAMES = {"dark": "Russet Dark", "light": "Russet Light",
         "studio-dark": "Russet Studio Dark (Xcode-style)", "studio-light": "Russet Studio Light (Xcode-style)"}


def luminance(hex_colour):
    def channel(c):
        c = c / 255
        return c / 12.92 if c <= 0.03928 else ((c + 0.055) / 1.055) ** 2.4
    r, g, b = (int(hex_colour[i:i + 2], 16) for i in (1, 3, 5))
    return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)


def contrast(a, b):
    la, lb = sorted((luminance(a), luminance(b)), reverse=True)
    return (la + 0.05) / (lb + 0.05)


def theme(kind):
    return studio(kind) if kind.startswith("studio") else theme_base(kind)


def theme_base(kind):
    p = PALETTES[kind]
    colors = {
        "foreground": p["fg"], "focusBorder": p["accent"], "descriptionForeground": p["muted"],
        "editor.background": p["bg"], "editor.foreground": p["fg"],
        "editor.lineHighlightBackground": p["lineHi"], "editor.selectionBackground": p["select"],
        "editor.findMatchBackground": p["find"], "editorCursor.foreground": p["accent"],
        "editorLineNumber.foreground": p["muted"], "editorLineNumber.activeForeground": p["fg"],
        "editorIndentGuide.background1": p["line"], "editorWhitespace.foreground": p["line"],
        "editorGroupHeader.tabsBackground": p["panel"], "editorWidget.background": p["raised"],
        "tab.activeBackground": p["bg"], "tab.inactiveBackground": p["panel"], "tab.activeForeground": p["fg"],
        "tab.inactiveForeground": p["muted"], "tab.activeBorderTop": p["accent"], "tab.border": p["line"],
        "sideBar.background": p["panel"], "sideBar.foreground": p["fg"], "sideBar.border": p["line"],
        "sideBarSectionHeader.background": p["panel"], "sideBarTitle.foreground": p["fg"],
        "activityBar.background": p["deep"], "activityBar.foreground": p["accent"],
        "activityBar.inactiveForeground": p["muted"], "activityBar.activeBorder": p["accent"],
        "activityBarBadge.background": p["accent"], "activityBarBadge.foreground": p["accentFg"],
        "titleBar.activeBackground": p["deep"], "titleBar.activeForeground": p["fg"],
        "titleBar.inactiveBackground": p["deep"], "titleBar.inactiveForeground": p["muted"],
        "statusBar.background": p["status"], "statusBar.foreground": p["statusFg"],
        "statusBar.noFolderBackground": p["status"], "statusBar.debuggingBackground": "#4F8A2E",
        "statusBar.debuggingForeground": "#FFFFFF",
        "panel.background": p["panel"], "panel.border": p["line"], "panelTitle.activeBorder": p["accent"],
        "terminal.background": p["bg"], "terminal.foreground": p["fg"],
        "button.background": p["accent"], "button.foreground": p["accentFg"],
        "button.secondaryBackground": p["raised"], "button.secondaryForeground": p["fg"],
        "input.background": p["raised"], "input.foreground": p["fg"], "input.border": p["line"],
        "dropdown.background": p["raised"], "dropdown.border": p["line"],
        "list.activeSelectionBackground": p["select"], "list.activeSelectionForeground": p["fg"],
        "list.hoverBackground": p["lineHi"], "list.inactiveSelectionBackground": p["lineHi"],
        "badge.background": p["accent"], "badge.foreground": p["accentFg"],
        "progressBar.background": p["accent"], "textLink.foreground": p["accent"],
        "textLink.activeForeground": p["keyword"],
        "editorError.foreground": p["error"], "editorWarning.foreground": p["warning"],
        "editorInfo.foreground": p["info"],
        "gitDecoration.modifiedResourceForeground": p["number"],
        "gitDecoration.untrackedResourceForeground": p["string"],
        "gitDecoration.deletedResourceForeground": p["error"],
    }
    rules = [
        (["comment", "punctuation.definition.comment"], p["comment"], "italic"),
        (["keyword", "storage", "storage.type", "keyword.control"], p["keyword"], None),
        (["string", "string.quoted", "markup.inline.raw"], p["string"], None),
        (["constant.numeric", "constant.language", "constant.character"], p["number"], None),
        (["entity.name.function", "support.function", "meta.function-call"], p["function"], None),
        (["entity.name.type", "entity.name.class", "support.type", "support.class", "entity.other.inherited-class"], p["type"], None),
        (["variable", "variable.parameter", "meta.definition.variable"], p["variable"], None),
        (["variable.other.constant", "entity.name.constant"], p["constant"], None),
        (["entity.name.tag"], p["tag"], None),
        (["entity.other.attribute-name"], p["attribute"], None),
        (["invalid"], p["error"], None),
        (["markup.heading"], p["keyword"], "bold"),
    ]
    token_colors = [
        {"scope": scope, "settings": {"foreground": fg, **({"fontStyle": style} if style else {})}}
        for scope, fg, style in rules
    ]
    semantic = {"type": p["type"], "class": p["type"], "struct": p["type"], "enum": p["type"],
                "interface": p["type"], "function": p["function"], "method": p["function"],
                "macro": p["keyword"], "parameter": p["variable"], "property": p["variable"],
                "enumMember": p["constant"], "*.mutable": {"underline": True}}
    return {
        "$schema": "vscode://schemas/color-theme",
        "name": NAMES[kind],
        "type": kind.removeprefix("studio-"),
        "colors": colors,
        "tokenColors": token_colors,
        "semanticHighlighting": True,
        "semanticTokenColors": semantic,
    }


def studio(kind):
    """Xcode-style: the base workbench colours on neutral greys, quieter chrome, Xcode's token roles."""
    p = PALETTES[kind]
    out = theme_base(kind)
    out["colors"].update({
        # The accent marks focus and selection only; title, tab and status bars stay grey.
        "activityBar.foreground": p["fg"], "tab.activeBorderTop": "#00000000", "tab.activeBorder": p["accent"],
        "statusBar.border": p["line"], "titleBar.border": p["line"], "activityBar.border": p["line"],
        "editorGroupHeader.tabsBorder": p["line"],
        "list.activeSelectionBackground": p["accent"], "list.activeSelectionForeground": p["accentFg"],
        "list.inactiveSelectionBackground": p["select"], "list.inactiveSelectionForeground": p["fg"],
        "editorCursor.foreground": p["fg"], "editorLink.activeForeground": p["url"],
        "textLink.activeForeground": p["accent"],
        # Xcode draws brackets as plain text, not in nesting colours.
        **{f"editorBracketHighlight.foreground{i}": p["fg"] for i in range(1, 7)},
        "editorBracketHighlight.unexpectedBracket.foreground": p["error"],
    })
    rules = [
        (["comment", "punctuation.definition.comment"], p["comment"], None),
        (["keyword", "storage", "storage.type", "storage.modifier", "keyword.control", "constant.language",
          "variable.language"], p["keyword"], "bold"),
        (["keyword.operator", "punctuation"], p["fg"], None),
        (["string", "string.quoted", "punctuation.definition.string"], p["string"], None),
        (["constant.numeric", "constant.character", "constant.character.escape"], p["number"], None),
        (["meta.preprocessor", "keyword.control.directive", "punctuation.definition.directive"], p["preprocessor"], None),
        (["storage.modifier.attribute", "storage.type.attribute", "entity.other.attribute-name", "meta.attribute",
          "punctuation.definition.attribute", "storage.type.annotation", "punctuation.definition.annotation",
          "variable.annotation"], p["attribute"], None),
        (["markup.underline.link", "string.other.link"], p["url"], None),
        (["entity.name.type.class", "entity.name.type.struct", "entity.name.type.enum", "entity.name.type.protocol",
          "entity.name.type.actor", "entity.name.type.extension", "entity.name.type.typealias"], p["typeDecl"], None),
        (["entity.name.function"], p["otherDecl"], None),
        (["entity.name.type", "entity.name.class", "entity.other.inherited-class"], p["type"], None),
        (["meta.function-call", "variable.function", "entity.name.function.call", "support.function.any-method"],
         p["function"], None),
        (["support.type", "support.class", "support.other.module"], p["sdkType"], None),
        (["support.function", "support.variable", "support.constant"], p["sdkFunction"], None),
        (["variable", "variable.parameter", "variable.other"], p["fg"], None),
        (["variable.other.constant", "entity.name.constant", "variable.other.enummember"], p["constant"], None),
        (["entity.name.tag"], p["keyword"], None),
        (["invalid"], p["error"], None),
        (["markup.heading", "entity.name.section"], p["heading"], "bold"),
        (["markup.bold"], p["fg"], "bold"),
        (["markup.italic"], p["fg"], "italic"),
    ]
    out["tokenColors"] = [
        {"scope": scope, "settings": {"foreground": fg, **({"fontStyle": style} if style else {})}}
        for scope, fg, style in rules
    ]
    # Language servers that mark declarations and SDK symbols get Xcode's split; the rest fall back.
    sdk = ("class", "struct", "type", "enum", "interface")
    out["semanticTokenColors"] = {
        **{t: p["type"] for t in sdk + ("typeParameter",)},
        **{t: p["function"] for t in ("function", "method", "property", "enumMember")},
        "macro": p["preprocessor"], "parameter": p["fg"], "variable": p["fg"], "namespace": p["sdkType"],
        "keyword": {"foreground": p["keyword"], "bold": True},
        "*.declaration": p["otherDecl"], "parameter.declaration": p["fg"],
        **{f"{t}.declaration": p["typeDecl"] for t in sdk},
        **{f"{t}.defaultLibrary": p["sdkType"] for t in sdk},
        **{f"{t}.defaultLibrary": p["sdkFunction"] for t in ("function", "method", "property", "enumMember")},
    }
    return out


def check(kind):
    p = PALETTES[kind]
    roles = ("fg", "muted", "comment", "keyword", "string", "number", "function", "type", "variable",
             "constant", "tag", "attribute", "error", "warning", "info", "accent")
    roles += tuple(k for k in ("preprocessor", "url", "typeDecl", "otherDecl", "sdkType", "sdkFunction", "heading") if k in p)
    pairs = [(k, p[k], p["bg"]) for k in roles]
    pairs += [("fg on panel", p["fg"], p["panel"]), ("muted on panel", p["muted"], p["panel"]),
              ("status", p["statusFg"], p["status"]), ("button", p["accentFg"], p["accent"])]
    failed = [(n, round(contrast(a, b), 2)) for n, a, b in pairs if contrast(a, b) < 4.5]
    return failed


def main():
    THEMES.mkdir(exist_ok=True)
    bad = False
    for kind in PALETTES:
        failed = check(kind)
        if failed:
            bad = True
            print(f"{kind}: below 4.5:1: {failed}")
        (THEMES / f"russet-{kind}-color-theme.json").write_text(
            json.dumps(theme(kind), indent=2) + "\n", encoding="utf-8", newline="\n")
    print("themes written" + (" (contrast problems above)" if bad else "; every text colour is AA"))
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
