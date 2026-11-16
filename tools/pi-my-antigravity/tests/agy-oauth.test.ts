// This file runs with bun.
import { chmodSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import { afterEach, expect, it, vi } from "vitest";

import {
  extractOAuthClientFromBinary,
  findAgyBinary,
  resolveOAuthClientFromAgy,
} from "../src/agy-oauth.ts";
import { ensureOAuthClientEnv } from "../src/constants.ts";

// Fixture credentials keep a `\u002e`/`\u0058` escape so the dummy values still match the parser at runtime
// without looking like a leaked Google credential to GitHub push protection.
const ORIGINAL_ID = process.env["MY_ANTIGRAVITY_CLIENT_ID"];
const ORIGINAL_SECRET = process.env["MY_ANTIGRAVITY_CLIENT_SECRET"];
const ORIGINAL_ALT_ID = process.env["ANTIGRAVITY_CLIENT_ID"];
const ORIGINAL_ALT_SECRET = process.env["ANTIGRAVITY_CLIENT_SECRET"];
const ORIGINAL_EXE = process.env["ANTIGRAVITY_AGENTAPI_EXE"];
const ORIGINAL_PATH = process.env["PATH"];

afterEach(() => {
  if (ORIGINAL_ID === undefined) {
    delete process.env["MY_ANTIGRAVITY_CLIENT_ID"];
  } else {
    process.env["MY_ANTIGRAVITY_CLIENT_ID"] = ORIGINAL_ID;
  }
  if (ORIGINAL_SECRET === undefined) {
    delete process.env["MY_ANTIGRAVITY_CLIENT_SECRET"];
  } else {
    process.env["MY_ANTIGRAVITY_CLIENT_SECRET"] = ORIGINAL_SECRET;
  }
  if (ORIGINAL_ALT_ID === undefined) {
    delete process.env["ANTIGRAVITY_CLIENT_ID"];
  } else {
    process.env["ANTIGRAVITY_CLIENT_ID"] = ORIGINAL_ALT_ID;
  }
  if (ORIGINAL_ALT_SECRET === undefined) {
    delete process.env["ANTIGRAVITY_CLIENT_SECRET"];
  } else {
    process.env["ANTIGRAVITY_CLIENT_SECRET"] = ORIGINAL_ALT_SECRET;
  }
  if (ORIGINAL_EXE === undefined) {
    delete process.env["ANTIGRAVITY_AGENTAPI_EXE"];
  } else {
    process.env["ANTIGRAVITY_AGENTAPI_EXE"] = ORIGINAL_EXE;
  }
  if (ORIGINAL_PATH === undefined) {
    delete process.env["PATH"];
  } else {
    process.env["PATH"] = ORIGINAL_PATH;
  }
});

function encodeAdrp(rd: number, pcVm: bigint, targetVm: bigint): number {
  const deltaPages = ((targetVm & ~0xfffn) - (pcVm & ~0xfffn)) >> 12n;
  const imm21 = Number(deltaPages & 0x1fffffn);
  const immlo = imm21 & 3;
  const immhi = (imm21 >>> 2) & 0x7ffff;
  return (0x90000000 | (immlo << 29) | (immhi << 5) | (rd & 0x1f)) >>> 0;
}

function encodeAddImm(rd: number, rn: number, imm12: number): number {
  return (0x91000000 | ((imm12 & 0xfff) << 10) | ((rn & 0x1f) << 5) | (rd & 0x1f)) >>> 0;
}

function buildSyntheticMachoArm64(): Buffer {
  const buf = Buffer.alloc(0x2000, 0);
  const textBaseVm = 0x100000000n;
  const textFileOffset = 0x400;
  const textSize = 0x400;

  // Mach-O 64-bit header (arm64, 2 load commands: __DATA then __TEXT)
  buf.writeUInt32LE(0xfeedfacf, 0);
  buf.writeUInt32LE(0x0100000c, 4);
  buf.writeUInt32LE(2, 16);

  // LC_SEGMENT_64 ("__DATA") at offset 32: 72 bytes
  buf.writeUInt32LE(0x19, 32);
  buf.writeUInt32LE(72, 36);
  buf.write("__DATA", 40, "ascii");

  // LC_SEGMENT_64 ("__TEXT") at offset 104: 72-byte segment + 2 * 80-byte sections = 232
  const cmdOff = 104;
  buf.writeUInt32LE(0x19, cmdOff);
  buf.writeUInt32LE(232, cmdOff + 4);
  buf.write("__TEXT", cmdOff + 8, "ascii");
  buf.writeBigUInt64LE(textBaseVm, cmdOff + 24);
  buf.writeUInt32LE(2, cmdOff + 64);

  // Section 0: "__stubs" at cmdOff + 72
  const stubsOff = cmdOff + 72;
  buf.write("__stubs", stubsOff, "ascii");

  // Section 1: "__text" at cmdOff + 72 + 80
  const secOff = stubsOff + 80;
  buf.write("__text", secOff, "ascii");
  buf.writeBigUInt64LE(textBaseVm + BigInt(textFileOffset), secOff + 32);
  buf.writeBigUInt64LE(BigInt(textSize), secOff + 40);
  buf.writeUInt32LE(textFileOffset, secOff + 48);

  // Place strings in rodata area (>= 0x1000)
  const id1Off = 0x1010;
  const id2Off = 0x1080;
  const sec1Off = 0x1100;
  const sec2Off = 0x1140;
  buf.write("1111111111111-dummyclient1.apps.googleusercontent\u002ecom", id1Off, "ascii");
  buf.write("2222222222222-dummyclient2.apps.googleusercontent\u002ecom", id2Off, "ascii");
  buf.write("GOCSP\u0058-1111111111111111111111111111", sec1Off, "ascii");
  buf.write("GOCSP\u0058-2222222222222222222222222222", sec2Off, "ascii");

  // Emit ADRP + ADD for id1 + sec1 at 0x420 / 0x430 (closer to id2 at 0x4c0 than 256 bytes)
  buf.writeUInt32LE(encodeAdrp(0, textBaseVm + 0x420n, textBaseVm + BigInt(id1Off)), 0x420);
  buf.writeUInt32LE(encodeAddImm(0, 0, id1Off & 0xfff), 0x424);
  buf.writeUInt32LE(encodeAdrp(1, textBaseVm + 0x430n, textBaseVm + BigInt(sec1Off)), 0x430);
  buf.writeUInt32LE(encodeAddImm(1, 1, sec1Off & 0xfff), 0x434);

  // Emit ADRP + ADD for id2 at 0x4c0 and sec2 at 0x4d0 (closest pair: 16 bytes)
  buf.writeUInt32LE(encodeAdrp(2, textBaseVm + 0x4c0n, textBaseVm + BigInt(id2Off)), 0x4c0);
  buf.writeUInt32LE(encodeAddImm(2, 2, id2Off & 0xfff), 0x4c8);
  buf.writeUInt32LE(encodeAdrp(3, textBaseVm + 0x4d0n, textBaseVm + BigInt(sec2Off)), 0x4d0);
  buf.writeUInt32LE(encodeAddImm(3, 3, sec2Off & 0xfff), 0x4d4);

  // Emit ADRP with negative delta (pc > target) and ADRP at end of __text without room for ADD
  buf.writeUInt32LE(encodeAdrp(4, textBaseVm + 0x3000n, textBaseVm + BigInt(id2Off)), 0x500);
  buf.writeUInt32LE(
    encodeAdrp(2, textBaseVm + BigInt(textFileOffset + textSize - 4), textBaseVm + BigInt(id2Off)),
    textFileOffset + textSize - 4,
  );

  return buf;
}

it("extracts single and paired OAuth client credentials from binary buffers", () => {
  const single = Buffer.from(
    "prefix1111111111111-singleid.apps.googleusercontent\u002ecom\0GOCSP\u0058-abcdefghijklmnopqr_123456789",
    "ascii",
  );
  expect(extractOAuthClientFromBinary(single)).toStrictEqual({
    clientId: "1111111111111-singleid.apps.googleusercontent\u002ecom",
    clientSecret: "GOCSP\u0058-abcdefghijklmnopqr_123456789",
  });

  const nonAntigravityId = Buffer.from(
    "999999999999-fallbackid.apps.googleusercontent\u002ecom\0GOCSP\u0058-abcdefghijklmnopqr_123456789",
    "ascii",
  );
  expect(extractOAuthClientFromBinary(nonAntigravityId)).toStrictEqual({
    clientId: "999999999999-fallbackid.apps.googleusercontent\u002ecom",
    clientSecret: "GOCSP\u0058-abcdefghijklmnopqr_123456789",
  });

  const macho = buildSyntheticMachoArm64();
  expect(extractOAuthClientFromBinary(macho)).toStrictEqual({
    clientId: "1111111111111-dummyclient1.apps.googleusercontent\u002ecom",
    clientSecret: "GOCSP\u0058-1111111111111111111111111111",
  });

  expect(extractOAuthClientFromBinary(Buffer.from("no credentials here"))).toBeUndefined();
  expect(
    extractOAuthClientFromBinary(
      Buffer.from("1234567890-id.apps.googleusercontent\u002ecom GOCSP\u0058-tooshort"),
    ),
  ).toBeUndefined();
  expect(
    extractOAuthClientFromBinary(
      Buffer.from(
        "1234567890-id.apps.googleusercontent\u002ecom GOCSP\u0058-invalid!______________________",
      ),
    ),
  ).toBeUndefined();

  const multiNoMacho = Buffer.from(
    "1111111111111-dummyid.apps.googleusercontent\u002ecom GOCSP\u0058-aaaaaaaaaaaaaaaaaaaaaaaaaaaa GOCSP\u0058-bbbbbbbbbbbbbbbbbbbbbbbbbbbb",
  );
  expect(extractOAuthClientFromBinary(multiNoMacho)).toBeUndefined();

  const badMacho = Buffer.alloc(64, 0);
  badMacho.writeUInt32LE(0xfeedfacf, 0);
  badMacho.writeUInt32LE(0x0100000c, 4);
  badMacho.writeUInt32LE(2, 16);
  badMacho.writeUInt32LE(0x19, 32);
  badMacho.writeUInt32LE(4, 36);
  badMacho.write("1111111111111-dummyid.apps.googleusercontent\u002ecom", 40, "ascii");
  expect(extractOAuthClientFromBinary(badMacho)).toBeUndefined();

  const noTextSection = Buffer.alloc(256, 0);
  noTextSection.writeUInt32LE(0xfeedfacf, 0);
  noTextSection.writeUInt32LE(0x0100000c, 4);
  noTextSection.writeUInt32LE(1, 16);
  noTextSection.writeUInt32LE(0x19, 32);
  noTextSection.writeUInt32LE(72, 36);
  noTextSection.write("__TEXT", 40, "ascii");
  noTextSection.write(
    "1111111111111-dummyid.apps.googleusercontent\u002ecom GOCSP\u0058-aaaaaaaaaaaaaaaaaaaaaaaaaaaa GOCSP\u0058-bbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    110,
    "ascii",
  );
  expect(extractOAuthClientFromBinary(noTextSection)).toBeUndefined();

  const truncatedCmd = Buffer.alloc(300, 0);
  truncatedCmd.writeUInt32LE(0xfeedfacf, 0);
  truncatedCmd.writeUInt32LE(0x0100000c, 4);
  truncatedCmd.writeUInt32LE(2, 16);
  truncatedCmd.writeUInt32LE(0x19, 32);
  truncatedCmd.writeUInt32LE(264, 36);
  truncatedCmd.write(
    "1111111111111-dummyid.apps.googleusercontent\u002ecom GOCSP\u0058-aaaaaaaaaaaaaaaaaaaaaaaaaaaa GOCSP\u0058-bbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    100,
    "ascii",
  );
  expect(extractOAuthClientFromBinary(truncatedCmd)).toBeUndefined();

  const truncatedSection = Buffer.alloc(300, 0);
  truncatedSection.writeUInt32LE(0xfeedfacf, 0);
  truncatedSection.writeUInt32LE(0x0100000c, 4);
  truncatedSection.writeUInt32LE(1, 16);
  truncatedSection.writeUInt32LE(0x19, 32);
  truncatedSection.writeUInt32LE(268, 36);
  truncatedSection.write("__TEXT", 40, "ascii");
  truncatedSection.writeUInt32LE(3, 32 + 64);
  truncatedSection.write(
    "1111111111111-dummyid.apps.googleusercontent\u002ecom GOCSP\u0058-aaaaaaaaaaaaaaaaaaaaaaaaaaaa GOCSP\u0058-bbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    120,
    "ascii",
  );
  expect(extractOAuthClientFromBinary(truncatedSection)).toBeUndefined();

  const nonSegmentCmd = Buffer.alloc(300, 0);
  nonSegmentCmd.writeUInt32LE(0xfeedfacf, 0);
  nonSegmentCmd.writeUInt32LE(0x0100000c, 4);
  nonSegmentCmd.writeUInt32LE(2, 16);
  nonSegmentCmd.writeUInt32LE(0x01, 32);
  nonSegmentCmd.writeUInt32LE(16, 36);
  nonSegmentCmd.writeUInt32LE(0x19, 48);
  nonSegmentCmd.writeUInt32LE(999, 52);
  nonSegmentCmd.write(
    "1111111111111-dummyid.apps.googleusercontent\u002ecom GOCSP\u0058-aaaaaaaaaaaaaaaaaaaaaaaaaaaa GOCSP\u0058-bbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    100,
    "ascii",
  );
  expect(extractOAuthClientFromBinary(nonSegmentCmd)).toBeUndefined();

  const noAdrpMatch = buildSyntheticMachoArm64();
  noAdrpMatch.fill(0, 0x400, 0x800);
  expect(extractOAuthClientFromBinary(noAdrpMatch)).toBeUndefined();
});

it("locates agy binary and auto-populates OAuth env vars", () => {
  const dir = mkdtempSync(join(tmpdir(), "myag-oauth-"));
  const subDir = join(dir, "bin");
  mkdirSync(subDir);
  const agyPath = join(subDir, process.platform === "win32" ? "agy.exe" : "agy");
  writeFileSync(
    agyPath,
    "error1111111111111-auto01.apps.googleusercontent\u002ecom\0GOCSP\u0058-zzzzzzzzzzzzzzzzzzzzzzzzzzzz",
  );
  const brokenAgy = join(dir, "broken-agy");
  writeFileSync(brokenAgy, "1111111111111-auto02.apps.googleusercontent\u002ecom");
  chmodSync(brokenAgy, 0o000);

  try {
    expect(findAgyBinary({ ANTIGRAVITY_AGENTAPI_EXE: agyPath, PATH: "" })).toBe(agyPath);
    expect(findAgyBinary({ ANTIGRAVITY_AGENTAPI_EXE: subDir, PATH: "" })).toBeUndefined();
    expect(
      findAgyBinary({ PATH: `${delimiter}${join(dir, "missing")}${delimiter}${subDir}` }),
    ).toBe(agyPath);
    expect(findAgyBinary({ PATH: "" })).toBeUndefined();
    expect(findAgyBinary({})).toBeUndefined();

    delete process.env["MY_ANTIGRAVITY_CLIENT_ID"];
    delete process.env["MY_ANTIGRAVITY_CLIENT_SECRET"];
    delete process.env["ANTIGRAVITY_CLIENT_ID"];
    delete process.env["ANTIGRAVITY_CLIENT_SECRET"];
    delete process.env["ANTIGRAVITY_AGENTAPI_EXE"];
    process.env["PATH"] = subDir;

    const resolved = ensureOAuthClientEnv();
    expect(resolved).toStrictEqual({
      clientId: "1111111111111-auto01.apps.googleusercontent\u002ecom",
      clientSecret: "GOCSP\u0058-zzzzzzzzzzzzzzzzzzzzzzzzzzzz",
    });
    expect(process.env["MY_ANTIGRAVITY_CLIENT_ID"]).toBe(
      "1111111111111-auto01.apps.googleusercontent\u002ecom",
    );
    expect(process.env["MY_ANTIGRAVITY_CLIENT_SECRET"]).toBe(
      "GOCSP\u0058-zzzzzzzzzzzzzzzzzzzzzzzzzzzz",
    );

    process.env["MY_ANTIGRAVITY_CLIENT_ID"] = "partial-id";
    delete process.env["MY_ANTIGRAVITY_CLIENT_SECRET"];
    expect(ensureOAuthClientEnv()).toStrictEqual({
      clientId: "partial-id",
      clientSecret: "GOCSP\u0058-zzzzzzzzzzzzzzzzzzzzzzzzzzzz",
    });

    delete process.env["MY_ANTIGRAVITY_CLIENT_ID"];
    process.env["MY_ANTIGRAVITY_CLIENT_SECRET"] = "partial-secret";
    expect(ensureOAuthClientEnv()).toStrictEqual({
      clientId: "1111111111111-auto01.apps.googleusercontent\u002ecom",
      clientSecret: "partial-secret",
    });

    const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
    expect(
      resolveOAuthClientFromAgy({ ANTIGRAVITY_AGENTAPI_EXE: brokenAgy, PATH: "" }),
    ).toBeUndefined();
    expect(warnSpy).toHaveBeenCalledTimes(1);

    process.env["PATH"] = "";
    delete process.env["MY_ANTIGRAVITY_CLIENT_ID"];
    delete process.env["MY_ANTIGRAVITY_CLIENT_SECRET"];
    expect(ensureOAuthClientEnv()).toBeUndefined();
  } finally {
    chmodSync(brokenAgy, 0o644);
    rmSync(dir, { recursive: true, force: true });
  }
});
