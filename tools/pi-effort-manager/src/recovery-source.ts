// This TypeScript file is executed with Bun.
export interface RecoverySource {
  readonly text: string;
  readonly omittedCodeUnits: number;
}
const MAX_FULL_CHUNKS = 128;
const RECOVERY_CHUNKS = 8;
const HEAD_DIVISOR = 4;

export function selectRecoverySource(text: string, chunkSize: number): RecoverySource {
  if (text.length <= chunkSize * MAX_FULL_CHUNKS) {
    return { text, omittedCodeUnits: 0 };
  }
  // Keep complete code points at both boundaries without allocating the whole transcript as an array.
  const headSize: number = Math.floor((chunkSize * RECOVERY_CHUNKS) / HEAD_DIVISOR);
  const tailSize: number = chunkSize * RECOVERY_CHUNKS - headSize;
  const head: string = text.slice(0, headSize).replace(/[\uD800-\uDBFF]$/u, "");
  const tail: string = text.slice(-tailSize).replace(/^[\uDC00-\uDFFF]/u, "");
  const omittedCodeUnits: number = text.length - head.length - tail.length;
  return {
    text: `${head}\n\n[Emergency recovery: ${String(omittedCodeUnits)} UTF-16 code units of historical middle omitted. Original session history is preserved. Do not infer completion or missing requirements from this gap.]\n\n${tail}`,
    omittedCodeUnits,
  };
}

export function recoveryNotice(omittedCodeUnits: number): string {
  return `Emergency recovery compaction: ${String(omittedCodeUnits)} UTF-16 code units of historical middle were omitted from the summary input. The original session history is preserved. Verify missing requirements and decisions from that history before acting on them.`;
}
