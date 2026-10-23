// Runs with bun: `bun run test` (vitest + happy-dom).
import { beforeEach, expect, test } from "vitest";
import {
  FEATURES,
  compilePatterns,
  detectFeatures,
  isInCall,
  isMeetingPath,
  readPage,
  resolveView,
} from "./meet-state.ts";
import englishPhrases from "../phrases/en.json" with { type: "json" };
import japanesePhrases from "../phrases/ja.json" with { type: "json" };

const patterns = compilePatterns([englishPhrases, japanesePhrases]);
const featureIds = (): string[] => detectFeatures({ text: readPage(document), patterns });

beforeEach(() => {
  document.body.innerHTML = "";
});

test("compilePatterns joins one language's phrases into a case-insensitive multiline pattern", () => {
  const compiled = compilePatterns([
    { recording: ["a", "^b$"], transcription: ["c"], gemini: ["d"] },
  ]);
  expect(compiled.recording.source).toBe("a|^b$");
  expect(compiled.recording.flags).toBe("imu");
  expect(compiled.transcription.source).toBe("c");
  expect(compiled.gemini.source).toBe("d");
});

test("compilePatterns combines the phrases of every language", () => {
  const compiled = compilePatterns([
    { recording: ["a"], transcription: ["c"], gemini: ["e"] },
    { recording: ["b"], transcription: ["d"], gemini: ["f"] },
  ]);
  expect(compiled.recording.source).toBe("a|b");
  expect(compiled.transcription.source).toBe("c|d");
  expect(compiled.gemini.source).toBe("e|f");
});

test("detectFeatures only knows the languages it is given", () => {
  document.body.innerHTML = "<div>録画中</div><div>REC</div>";
  expect(
    detectFeatures({ text: readPage(document), patterns: compilePatterns([japanesePhrases]) }),
  ).toStrictEqual(["recording"]);
  document.body.innerHTML = "<div>録画中</div>";
  expect(
    detectFeatures({ text: readPage(document), patterns: compilePatterns([englishPhrases]) }),
  ).toStrictEqual([]);
});

test("isMeetingPath accepts meeting codes and rejects other Meet pages", () => {
  expect(isMeetingPath("/nxm-khwo-bro")).toBe(true);
  expect(isMeetingPath("/meet/rxq-jsqy-hdf")).toBe(true);
  expect(isMeetingPath("/home")).toBe(false);
  expect(isMeetingPath("/")).toBe(false);
});

test("isInCall detects the leave-call icon regardless of the button label language", () => {
  document.body.innerHTML = '<button aria-label="Quitter l\'appel"><i>call_end</i></button>';
  expect(isInCall(readPage(document))).toBe(true);
});

test("isInCall detects the leave-call icon nested below the button", () => {
  document.body.innerHTML = "<button><span><i>call_end</i></span></button>";
  expect(isInCall(readPage(document))).toBe(true);
});

test("isInCall ignores the pre-join screen icons", () => {
  document.body.innerHTML =
    "<button><i>mic</i></button><button><i>videocam</i></button><button>今すぐ参加</button>";
  expect(isInCall(readPage(document))).toBe(false);
});

test("isInCall ignores the localized leave-call label without the icon", () => {
  document.body.innerHTML = '<button aria-label="通話から退出"></button>';
  expect(isInCall(readPage(document))).toBe(false);
});

test("isInCall ignores call_end inside a longer line", () => {
  document.body.innerHTML = "<p>Press call_end to leave</p>";
  expect(isInCall(readPage(document))).toBe(false);
});

test("isInCall ignores a button element that only wraps the icon text with other text", () => {
  document.body.innerHTML = "<button><span>call_end<b>end</b></span></button>";
  expect(isInCall(readPage(document))).toBe(false);
});

test("detectFeatures finds nothing on an empty page", () => {
  expect(featureIds()).toStrictEqual([]);
});

test("detectFeatures reads the pre-join recording notice", () => {
  document.body.innerHTML = "<div>このビデオ会議は録画されています</div>";
  expect(featureIds()).toStrictEqual(["recording"]);
});

test("detectFeatures reads the scheduled recording and Gemini notices", () => {
  document.body.innerHTML = "<div>Meet は録画を行います</div><div>Gemini がメモを作成します</div>";
  expect(featureIds()).toStrictEqual(["recording", "gemini"]);
});

test("detectFeatures reads an in-call Gemini notes label", () => {
  document.body.innerHTML = '<div aria-label="Gemini がメモを作成しています"></div>';
  expect(featureIds()).toStrictEqual(["gemini"]);
});

test("detectFeatures ignores the Recording item title in the English meeting tools", () => {
  document.body.innerHTML = "<div>Recording</div><div>Record your meeting</div>";
  expect(featureIds()).toStrictEqual([]);
});

test("detectFeatures reads a data-tooltip next to an unrelated aria-label", () => {
  document.body.innerHTML =
    '<div aria-label="Meeting details" data-tooltip="This call is being recorded"></div>';
  expect(featureIds()).toStrictEqual(["recording"]);
});

test("detectFeatures reads a data-tooltip", () => {
  document.body.innerHTML = '<div data-tooltip="This call is being recorded"></div>';
  expect(featureIds()).toStrictEqual(["recording"]);
});

test("detectFeatures matches a standalone REC badge line", () => {
  document.body.innerHTML = "<div>REC</div>";
  expect(featureIds()).toStrictEqual(["recording"]);
});

test("detectFeatures reads transcription with recording and Gemini in priority order", () => {
  document.body.innerHTML =
    "<div>Gemini がメモを作成しています</div><div>文字起こし中</div><div>録画中</div>";
  expect(featureIds()).toStrictEqual(["recording", "transcription", "gemini"]);
});

test("detectFeatures reads English indicators", () => {
  document.body.innerHTML =
    "<div>REC</div><div>This call is being transcribed</div><div>Gemini is taking notes</div>";
  expect(featureIds()).toStrictEqual(["recording", "transcription", "gemini"]);
});

test("detectFeatures ignores the start-recording tool and the Gemini start offer", () => {
  document.body.innerHTML =
    '<div aria-label="録画 会議を記録します"></div><div aria-label="文字起こしを開始"></div>' +
    "<div>Gemini を使用したメモの作成</div><div>会議終了後に録画の作成や字幕データの保存は行われません。</div>";
  expect(featureIds()).toStrictEqual([]);
});

test("detectFeatures ignores script, style and hidden text", () => {
  document.body.innerHTML =
    '<script>const label = "録画中";</script><style>.rec::after { content: "録画中"; }</style>' +
    '<div style="display:none">録画中</div>';
  expect(featureIds()).toStrictEqual([]);
});

test("detectFeatures does not treat REC inside a longer line as the badge", () => {
  document.body.innerHTML = "<div>RECENT</div><div>Start REC now</div>";
  expect(featureIds()).toStrictEqual([]);
});

test("detectFeatures ignores elements without a usable label", () => {
  document.body.innerHTML = '<div aria-label=""></div><svg data-tooltip="録画中"></svg>';
  expect(featureIds()).toStrictEqual([]);
});

test("resolveView shows the loading view before joining", () => {
  expect(resolveView({ active: [], hint: [], inCall: false, msSinceJoin: 0 })).toStrictEqual({
    tone: "loading",
    messages: ["loading"],
  });
});

test("resolveView shows the idle view in a call without indicators", () => {
  expect(resolveView({ active: [], hint: [], inCall: true, msSinceJoin: 20_000 })).toStrictEqual({
    tone: "idle",
    messages: ["idle"],
  });
});

test("resolveView keeps the recording tone when Gemini also runs", () => {
  expect(
    resolveView({
      active: FEATURES.filter((feature) => feature !== "transcription"),
      hint: [],
      inCall: true,
      msSinceJoin: 20_000,
    }),
  ).toStrictEqual({ tone: "recording", messages: ["recording", "gemini"] });
});

test("resolveView shows the transcription tone without recording", () => {
  expect(
    resolveView({
      active: FEATURES.filter((feature) => feature !== "recording"),
      hint: [],
      inCall: true,
      msSinceJoin: 20_000,
    }),
  ).toStrictEqual({ tone: "transcription", messages: ["transcription", "gemini"] });
});

test("resolveView merges the pre-join hint in priority order during the join grace period", () => {
  expect(
    resolveView({
      active: ["gemini"],
      hint: ["recording"],
      inCall: true,
      msSinceJoin: 9999,
    }),
  ).toStrictEqual({ tone: "recording", messages: ["recording", "gemini"] });
});

test("resolveView drops the pre-join hint after the join grace period", () => {
  expect(
    resolveView({
      active: [],
      hint: ["recording"],
      inCall: true,
      msSinceJoin: 10_000,
    }),
  ).toStrictEqual({ tone: "idle", messages: ["idle"] });
});

test("readPage joins labels, tooltips and visible lines into one string", () => {
  document.body.innerHTML =
    '<div aria-label="Info" data-tooltip="Recorded"></div><div>REC</div><div style="display:none">secret</div>';
  expect(readPage(document)).toBe("Info\nRecorded\nREC");
});
