// This file runs with bun.
import { existsSync, readFileSync, statSync } from "node:fs";
import { delimiter, join } from "node:path";

import type { OAuthClientConfig } from "./constants.ts";

interface BinaryMatch {
  offset: number;
  value: string;
}

interface TextSection {
  fileOffset: number;
  size: number;
  virtualAddress: bigint;
}

interface MachoTextSegment {
  text: TextSection;
  textBaseVirtualAddress: bigint;
}

interface PageTarget {
  offset: number;
  page: bigint;
  pageOffset: number;
  sites: number[];
}

interface SecretTarget {
  secret: BinaryMatch;
  target: PageTarget;
}

interface AddImmediateQuery {
  instruction: number;
  register: number;
  pageOffset: number;
}

interface TextReferencesQuery {
  buffer: Buffer;
  text: TextSection;
  targets: readonly PageTarget[];
}

interface TargetSiteCheck {
  buffer: Buffer;
  adrpPosition: number;
  end: number;
  register: number;
  targets: readonly PageTarget[];
}

interface InstructionInspectQuery {
  buffer: Buffer;
  text: TextSection;
  position: number;
  end: number;
  targetsByPage: ReadonlyMap<bigint, readonly PageTarget[]>;
}

interface FollowingAddQuery {
  buffer: Buffer;
  adrpPosition: number;
  end: number;
  register: number;
  pageOffset: number;
}

interface TextSectionQuery {
  buffer: Buffer;
  startOffset: number;
  remainingSections: number;
}

interface PairedSecretQuery {
  buffer: Buffer;
  clientIdOffset: number;
  secrets: readonly BinaryMatch[];
}

interface BinarySearchQuery {
  buffer: Buffer;
  searchFrom: number;
  matches: BinaryMatch[];
}

interface LoadCommandScanQuery {
  buffer: Buffer;
  commandOffset: number;
  remainingCommands: number;
}

interface ClosestSecretQuery {
  secretTargets: readonly SecretTarget[];
  idSites: readonly number[];
  index: number;
  bestSecret: BinaryMatch | undefined;
  bestDistance: number;
}

const CLIENT_ID_NEEDLE = Buffer.from(".apps.googleusercontent.com");
const CLIENT_SECRET_NEEDLE = Buffer.from("GOCSPX-");
const CLIENT_ID_LOOKBACK_BYTES = 96;
const CLIENT_SECRET_LENGTH = 35;
const CLIENT_ID_PATTERN = /(\d{10,}-[a-z0-9]+\.apps\.googleusercontent\.com)$/;
const CLIENT_SECRET_PATTERN = /^GOCSPX-[A-Za-z0-9_-]{28}$/;
const ANTIGRAVITY_CLIENT_PREFIX = "1071006060591-";

const MACHO_MAGIC_64 = 0xfeedfacf;
const CPU_TYPE_ARM64 = 0x0100000c;
const LC_SEGMENT_64 = 0x19;
const MACHO_HEADER_SIZE = 32;
const MACHO_CPU_TYPE_OFFSET = 4;
const MACHO_COMMAND_COUNT_OFFSET = 16;
const LOAD_COMMAND_HEADER_SIZE = 8;
const LOAD_COMMAND_SIZE_OFFSET = 4;
const SEGMENT_HEADER_SIZE = 72;
const SEGMENT_NAME_OFFSET = 8;
const SEGMENT_VMADDR_OFFSET = 24;
const SEGMENT_SECTION_COUNT_OFFSET = 64;
const SECTION_HEADER_SIZE = 80;
const SECTION_VMADDR_OFFSET = 32;
const SECTION_SIZE_OFFSET = 40;
const SECTION_FILE_OFFSET = 48;
const MACHO_NAME_LENGTH = 16;
const PAGE_MASK = ~0xfffn;
const PAGE_OFFSET_MASK = 0xfffn;
const PAGE_OFFSET_BITS_MASK = 0xfff;
const ADRP_MASK = 0x9f000000;
const ADRP_OP = 0x90000000;
const ADRP_IMMLO_SHIFT = 29;
const ADRP_IMMLO_MASK = 3;
const ADRP_IMMHI_SHIFT = 5;
const ADRP_IMMHI_MASK = 0x7ffff;
const ADRP_SIGN_BIT = 1n << 20n;
const ADRP_SIGN_EXTEND = 1n << 21n;
const ADRP_PAGE_SHIFT = 12n;
const ADD_IMM_MASK = 0xffc00000;
const ADD_IMM_OP = 0x91000000;
const ADD_RN_SHIFT = 5;
const ADD_IMM12_SHIFT = 10;
const REGISTER_MASK = 0x1f;
const MAX_ADRP_ADD_GAP = 6;
const MAX_PAIR_DISTANCE_BYTES = 256;
const INSTRUCTION_BYTES = 4;
const ADRP_ADD_OFFSETS: readonly number[] = Array.from(
  { length: MAX_ADRP_ADD_GAP },
  (_, index) => (index + 1) * INSTRUCTION_BYTES,
);
const AGY_BINARY_NAME = process.platform === "win32" ? "agy.exe" : "agy";

const isExistingFile = (filePath: string): boolean =>
  existsSync(filePath) && statSync(filePath).isFile();

const readAsciiName = (buffer: Buffer, offset: number): string =>
  buffer.toString("ascii", offset, offset + MACHO_NAME_LENGTH).replace(/\0.*$/, "");

const matchesAddImmediate = ({ instruction, register, pageOffset }: AddImmediateQuery): boolean =>
  (instruction & ADD_IMM_MASK) >>> 0 === ADD_IMM_OP &&
  ((instruction >>> ADD_RN_SHIFT) & REGISTER_MASK) === register &&
  ((instruction >>> ADD_IMM12_SHIFT) & PAGE_OFFSET_BITS_MASK) === pageOffset;

const matchesAddAt = (
  nextPosition: number,
  { buffer, end, register, pageOffset }: FollowingAddQuery,
): boolean =>
  nextPosition <= end &&
  matchesAddImmediate({
    instruction: buffer.readUInt32LE(nextPosition),
    register,
    pageOffset,
  });

const hasFollowingAdd = (query: FollowingAddQuery): boolean =>
  ADRP_ADD_OFFSETS.some((byteOffset) => matchesAddAt(query.adrpPosition + byteOffset, query));

const minimumPairDistance = (idSites: readonly number[], secretSites: readonly number[]): number =>
  Math.min(
    ...idSites.flatMap((idPosition) =>
      secretSites.map((secretPosition) => Math.abs(idPosition - secretPosition)),
    ),
  );

function findAgyInPath(pathValue: string): string | undefined {
  for (const directory of pathValue.split(delimiter)) {
    const trimmed = directory.trim();
    if (trimmed.length === 0) {
      continue;
    }
    const candidate = join(trimmed, AGY_BINARY_NAME);
    if (isExistingFile(candidate)) {
      return candidate;
    }
  }
  return undefined;
}

/** Locate the installed `agy` CLI binary from `ANTIGRAVITY_AGENTAPI_EXE` or `PATH`. */
export function findAgyBinary(environment: NodeJS.ProcessEnv): string | undefined {
  const explicit = environment["ANTIGRAVITY_AGENTAPI_EXE"]?.trim();
  return explicit && isExistingFile(explicit) ? explicit : findAgyInPath(environment["PATH"] ?? "");
}

function collectClientIds({
  buffer,
  searchFrom,
  matches,
}: BinarySearchQuery): readonly BinaryMatch[] {
  const position = buffer.indexOf(CLIENT_ID_NEEDLE, searchFrom);
  if (position === -1) {
    return matches;
  }
  const end = position + CLIENT_ID_NEEDLE.length;
  const sliceStart = Math.max(0, position - CLIENT_ID_LOOKBACK_BYTES);
  const match = CLIENT_ID_PATTERN.exec(buffer.toString("ascii", sliceStart, end));
  const matchedId = match?.[1];
  if (match !== null && matchedId !== undefined) {
    matches.push({ offset: sliceStart + match.index, value: matchedId });
  }
  return collectClientIds({ buffer, searchFrom: end, matches });
}

const readClientSecretAt = (buffer: Buffer, position: number): string | undefined =>
  position + CLIENT_SECRET_LENGTH <= buffer.length
    ? CLIENT_SECRET_PATTERN.exec(
        buffer.toString("ascii", position, position + CLIENT_SECRET_LENGTH),
      )?.[0]
    : undefined;

function collectClientSecrets({
  buffer,
  searchFrom,
  matches,
}: BinarySearchQuery): readonly BinaryMatch[] {
  const position = buffer.indexOf(CLIENT_SECRET_NEEDLE, searchFrom);
  if (position === -1) {
    return matches;
  }
  const value = readClientSecretAt(buffer, position);
  if (value !== undefined) {
    matches.push({ offset: position, value });
  }
  return collectClientSecrets({
    buffer,
    searchFrom: position + CLIENT_SECRET_NEEDLE.length,
    matches,
  });
}

const readTextSectionAt = (buffer: Buffer, startOffset: number): TextSection => ({
  virtualAddress: buffer.readBigUInt64LE(startOffset + SECTION_VMADDR_OFFSET),
  size: Number(buffer.readBigUInt64LE(startOffset + SECTION_SIZE_OFFSET)),
  fileOffset: buffer.readUInt32LE(startOffset + SECTION_FILE_OFFSET),
});

function scanTextSections({
  buffer,
  startOffset,
  remainingSections,
}: TextSectionQuery): TextSection | undefined {
  if (remainingSections <= 0 || startOffset + SECTION_HEADER_SIZE > buffer.length) {
    return undefined;
  }
  return readAsciiName(buffer, startOffset) === "__text"
    ? readTextSectionAt(buffer, startOffset)
    : scanTextSections({
        buffer,
        startOffset: startOffset + SECTION_HEADER_SIZE,
        remainingSections: remainingSections - 1,
      });
}

function readTextSegment(buffer: Buffer, commandOffset: number): MachoTextSegment | undefined {
  if (
    buffer.readUInt32LE(commandOffset) !== LC_SEGMENT_64 ||
    commandOffset + SEGMENT_HEADER_SIZE > buffer.length
  ) {
    return undefined;
  }
  if (readAsciiName(buffer, commandOffset + SEGMENT_NAME_OFFSET) !== "__TEXT") {
    return undefined;
  }
  const section = scanTextSections({
    buffer,
    startOffset: commandOffset + SEGMENT_HEADER_SIZE,
    remainingSections: buffer.readUInt32LE(commandOffset + SEGMENT_SECTION_COUNT_OFFSET),
  });
  return section
    ? {
        text: section,
        textBaseVirtualAddress: buffer.readBigUInt64LE(commandOffset + SEGMENT_VMADDR_OFFSET),
      }
    : undefined;
}

function scanLoadCommands({
  buffer,
  commandOffset,
  remainingCommands,
}: LoadCommandScanQuery): MachoTextSegment | undefined {
  if (remainingCommands <= 0 || commandOffset + LOAD_COMMAND_HEADER_SIZE > buffer.length) {
    return undefined;
  }
  const commandSize = buffer.readUInt32LE(commandOffset + LOAD_COMMAND_SIZE_OFFSET);
  if (commandSize < LOAD_COMMAND_HEADER_SIZE || commandOffset + commandSize > buffer.length) {
    return undefined;
  }
  return (
    readTextSegment(buffer, commandOffset) ??
    scanLoadCommands({
      buffer,
      commandOffset: commandOffset + commandSize,
      remainingCommands: remainingCommands - 1,
    })
  );
}

const isArm64MachoHeader = (buffer: Buffer): boolean =>
  buffer.length >= MACHO_HEADER_SIZE &&
  buffer.readUInt32LE(0) === MACHO_MAGIC_64 &&
  buffer.readUInt32LE(MACHO_CPU_TYPE_OFFSET) === CPU_TYPE_ARM64;

const findMachoTextSection = (buffer: Buffer): MachoTextSegment | undefined =>
  isArm64MachoHeader(buffer)
    ? scanLoadCommands({
        buffer,
        commandOffset: MACHO_HEADER_SIZE,
        remainingCommands: buffer.readUInt32LE(MACHO_COMMAND_COUNT_OFFSET),
      })
    : undefined;

function toPageTarget(offset: number, textBaseVirtualAddress: bigint): PageTarget {
  const virtualAddress = textBaseVirtualAddress + BigInt(offset);
  return {
    offset,
    page: virtualAddress & PAGE_MASK,
    pageOffset: Number(virtualAddress & PAGE_OFFSET_MASK),
    sites: [],
  };
}

const isAdrpInstruction = (instruction: number): boolean =>
  (instruction & ADRP_MASK) >>> 0 === ADRP_OP;

function decodeAdrpTargetPage(instruction: number, programCounterVirtualAddress: bigint): bigint {
  const rawImmediate = BigInt(
    (((instruction >>> ADRP_IMMHI_SHIFT) & ADRP_IMMHI_MASK) << 2) |
      ((instruction >>> ADRP_IMMLO_SHIFT) & ADRP_IMMLO_MASK),
  );
  const signedImmediate =
    rawImmediate & ADRP_SIGN_BIT ? rawImmediate - ADRP_SIGN_EXTEND : rawImmediate;
  return (programCounterVirtualAddress & PAGE_MASK) + (signedImmediate << ADRP_PAGE_SHIFT);
}

function groupTargetsByPage(targets: readonly PageTarget[]): Map<bigint, PageTarget[]> {
  const grouped = new Map<bigint, PageTarget[]>();
  for (const target of targets) {
    const bucket = grouped.get(target.page) ?? [];
    bucket.push(target);
    grouped.set(target.page, bucket);
  }
  return grouped;
}

const recordMatchingTargetSites = ({
  buffer,
  adrpPosition,
  end,
  register,
  targets,
}: TargetSiteCheck): void =>
  targets
    .filter((target) =>
      hasFollowingAdd({
        buffer,
        adrpPosition,
        end,
        register,
        pageOffset: target.pageOffset,
      }),
    )
    .forEach((target) => {
      target.sites.push(adrpPosition);
    });

function inspectTextInstruction({
  buffer,
  text,
  position,
  end,
  targetsByPage,
}: InstructionInspectQuery): void {
  const instruction = buffer.readUInt32LE(position);
  if (!isAdrpInstruction(instruction)) {
    return;
  }
  const targetPage = decodeAdrpTargetPage(
    instruction,
    text.virtualAddress + BigInt(position - text.fileOffset),
  );
  const matchingTargets = targetsByPage.get(targetPage);
  if (matchingTargets) {
    recordMatchingTargetSites({
      buffer,
      adrpPosition: position,
      end,
      register: instruction & REGISTER_MASK,
      targets: matchingTargets,
    });
  }
}

function collectArm64TextReferences({ buffer, text, targets }: TextReferencesQuery): void {
  const end = Math.min(buffer.length, text.fileOffset + text.size) - INSTRUCTION_BYTES;
  const targetsByPage = groupTargetsByPage(targets);
  for (let position = text.fileOffset; position <= end; position += INSTRUCTION_BYTES) {
    inspectTextInstruction({ buffer, text, position, end, targetsByPage });
  }
}

function pickClosestSecret({
  secretTargets,
  idSites,
  index,
  bestSecret,
  bestDistance,
}: ClosestSecretQuery): BinaryMatch | undefined {
  const entry = secretTargets[index];
  if (!entry) {
    return bestSecret;
  }
  const distance = minimumPairDistance(idSites, entry.target.sites);
  return distance < bestDistance
    ? pickClosestSecret({
        secretTargets,
        idSites,
        index: index + 1,
        bestSecret: entry.secret,
        bestDistance: distance,
      })
    : pickClosestSecret({
        secretTargets,
        idSites,
        index: index + 1,
        bestSecret,
        bestDistance,
      });
}

function findPairedSecret({
  buffer,
  clientIdOffset,
  secrets,
}: PairedSecretQuery): BinaryMatch | undefined {
  const machO = findMachoTextSection(buffer);
  if (!machO) {
    return undefined;
  }
  const idTarget = toPageTarget(clientIdOffset, machO.textBaseVirtualAddress);
  const secretTargets = secrets.map((secret) => ({
    secret,
    target: toPageTarget(secret.offset, machO.textBaseVirtualAddress),
  }));
  collectArm64TextReferences({
    buffer,
    text: machO.text,
    targets: [idTarget, ...secretTargets.map((entry) => entry.target)],
  });
  return pickClosestSecret({
    secretTargets,
    idSites: idTarget.sites,
    index: 0,
    bestSecret: undefined,
    bestDistance: MAX_PAIR_DISTANCE_BYTES + 1,
  });
}

/** Extract Antigravity's OAuth client ID and secret from a compiled `agy` binary buffer. */
export function extractOAuthClientFromBinary(buffer: Buffer): OAuthClientConfig | undefined {
  const ids = collectClientIds({ buffer, searchFrom: 0, matches: [] });
  const [firstId] = ids;
  if (!firstId) {
    return undefined;
  }
  const secrets = collectClientSecrets({ buffer, searchFrom: 0, matches: [] });
  const [firstSecret] = secrets;
  if (!firstSecret) {
    return undefined;
  }
  const selectedId =
    ids.find((item) => item.value.startsWith(ANTIGRAVITY_CLIENT_PREFIX)) ?? firstId;
  const selectedSecret =
    secrets.length === 1
      ? firstSecret
      : findPairedSecret({ buffer, clientIdOffset: selectedId.offset, secrets });
  return selectedSecret
    ? { clientId: selectedId.value, clientSecret: selectedSecret.value }
    : undefined;
}

/** Resolve OAuth client credentials from the installed `agy` binary when present. */
export function resolveOAuthClientFromAgy(
  environment: NodeJS.ProcessEnv,
): OAuthClientConfig | undefined {
  const binaryPath = findAgyBinary(environment);
  if (!binaryPath) {
    return undefined;
  }
  try {
    return extractOAuthClientFromBinary(readFileSync(binaryPath));
  } catch (error) {
    console.warn(`my-antigravity: failed to read agy binary at ${binaryPath}: ${String(error)}`);
    return undefined;
  }
}
