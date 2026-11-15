// `displayText`: page text shown in LocalPass's panel (site names, account names) loses every character that has no
// width or that reorders or hides the text around it (8.6 F4: the same set as lp-broker's `is_invisible`).

import { describe, expect, test } from "bun:test";
import { displayText } from "../src/shared/limits";

describe("displayText", () => {
  test("drops direction marks, zero-width characters and the byte order mark (as before)", () => {
    expect(displayText("a‮b​c﻿d⁦e")).toBe("abcde");
    expect(displayText("؜‏x­y")).toBe("xy");
    expect(displayText("Café 日本語")).toBe("Café 日本語");
  });

  test("drops the tag characters (hidden text after a visible name)", () => {
    expect(displayText("a\u{E0001}\u{E0068}\u{E0069}\u{E007F}b")).toBe("ab");
    expect(displayText("a\u{E0000}b\u{E0020}c")).toBe("abc");
  });

  test("line and paragraph separators become a space", () => {
    expect(displayText("a b c")).toBe("a b c");
  });

  test("drops the Hangul fillers and the combining grapheme joiner: a name of blank letters shows as nothing", () => {
    expect(displayText("ᅟᅠaㅤbﾠc͏d")).toBe("abcd");
    expect(displayText("ㅤㅤㅤ")).toBe("");
  });

  test("drops every other format character", () => {
    for (const c of ["؀", "؅", "۝", "܏", "࢐", "࢑", "࣢", "\u{110BD}", "\u{110CD}", "\u{13430}", "\u{1343F}", "\u{1BCA0}", "\u{1BCA3}", "\u{1D173}", "\u{1D17A}"]) {
      expect(displayText(`x${c}y`), c.codePointAt(0)!.toString(16)).toBe("xy");
    }
    // Their neighbours stay, and so do the halves of other astral characters (the pattern works on code points).
    expect(displayText("؆ᅡㅥ\u{E0080}😀")).toBe("؆ᅡㅥ\u{E0080}😀");
  });

  test("cuts by characters, not UTF-16 units", () => {
    expect(displayText("😀".repeat(5), 3)).toBe("😀😀…");
  });
});
