// Runs with Bun. Executor owns OAuth; only authenticated ciphertext crosses MCP.
import { createHmac, randomUUID, timingSafeEqual } from "node:crypto";
import { z } from "zod";
import type { RemoteValue } from "./engine";
import { command, requiredCommand } from "./io";
import {
  check,
  type Envelope,
  parseEnvelope,
  type Snapshot,
  stable,
} from "./model";
import { type Profile, saveProfile } from "./vault";

export interface McpOptions {
  profile: Profile;
  device: string;
  age: string;
  executor: string;
}
interface Part {
  total: number;
  mac: string;
  part: string;
}
const LIMIT: number = 65536;
const PART: number = 6000;
const partSchema = z
  .object({
    total: z.number().int().min(1).max(90000),
    mac: z.string().regex(/^[a-f0-9]{64}$/),
    part: z.string().max(PART),
  })
  .strict();
const signedSchema = z
  .object({
    version: z.literal(2),
    data: z
      .string()
      .min(1)
      .max(90000)
      .regex(
        /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/,
      ),
    mac: z.string().regex(/^[a-f0-9]{64}$/),
  })
  .strict();
const outerSchema = z.object({
  ok: z.literal(true),
  data: z.object({
    isError: z.literal(false).optional(),
    content: z
      .array(z.object({ type: z.literal("text"), text: z.string() }))
      .length(1),
  }),
});
const listSchema = z.object({
  items: z.array(z.object({ key: z.string().max(1024) })).max(1000),
  truncated: z.literal(false),
});

export class McpStorage {
  readonly options: McpOptions;
  readonly client = { destroy: (): void => undefined };
  constructor(options: McpOptions) {
    this.options = options;
  }
  async call(code: string): Promise<unknown> {
    const output: Buffer = await requiredCommand({
      executable: this.options.executor,
      args: [
        "call",
        "cloudflare-api",
        "user",
        "default",
        "execute",
        JSON.stringify({ code }),
      ],
      input: "",
    });
    const parsed = outerSchema.safeParse(JSON.parse(output.toString()));
    check(
      parsed.success,
      "Executor MCP failed or requires approval; no data was imported",
    );
    const text: string | undefined = parsed.data.data.content[0]?.text;
    check(text, "MCP result missing");
    return JSON.parse(text);
  }
  async verifyEndpoint(): Promise<void> {
    const output: Buffer = await requiredCommand({
      executable: this.options.executor,
      args: [
        "call",
        "executor",
        "mcp",
        "getServer",
        '{"slug":"cloudflare-api"}',
      ],
      input: "",
    });
    const result = z
      .object({
        ok: z.literal(true),
        data: z.object({
          integration: z.object({
            config: z.object({
              endpoint: z.literal("https://mcp.cloudflare.com/mcp"),
            }),
          }),
        }),
      })
      .safeParse(JSON.parse(output.toString()));
    check(result.success, "Sync requires the official Cloudflare MCP endpoint");
  }
  path(key: string): string {
    check(
      /^(latest\.age|history\/[0-9TZ.:-]+-[a-f0-9-]+\.age)$/.test(key),
      "Invalid object key",
    );
    const config = this.options.profile.config;
    return `/accounts/${config.accountId}/r2/buckets/${config.bucket}/objects/${encodeURIComponent(`v1/${config.group}/${key}`)}`;
  }
  mac(data: string): string {
    const config = this.options.profile.config;
    return createHmac(
      "sha256",
      Buffer.from(this.options.profile.authKey, "hex"),
    )
      .update(
        stable([
          "executor-sync-v2",
          config.accountId,
          config.bucket,
          config.group,
          data,
        ]),
      )
      .digest("hex");
  }
  async encrypt(envelope: Envelope): Promise<Buffer> {
    const text: string = stable(envelope);
    check(
      Buffer.byteLength(text) <= LIMIT - 4096,
      "Snapshot exceeds secure MCP transport limit",
    );
    const result = await command({
      executable: this.options.age,
      args: ["-r", this.options.profile.config.ageRecipient],
      input: text,
      identity: null,
    });
    check(
      result.code === 0 && result.output.length <= LIMIT,
      "Snapshot encryption failed or exceeds limit",
    );
    const data: string = result.output.toString("base64");
    return Buffer.from(
      JSON.stringify({ version: 2, data, mac: this.mac(data) }),
    );
  }
  async decrypt(bytes: Uint8Array): Promise<Envelope> {
    check(bytes.byteLength <= 100000, "Encrypted object too large");
    const parsed = signedSchema.safeParse(
      JSON.parse(Buffer.from(bytes).toString()),
    );
    check(parsed.success, "Invalid authenticated ciphertext");
    check(
      timingSafeEqual(
        Buffer.from(parsed.data.mac, "hex"),
        Buffer.from(this.mac(parsed.data.data), "hex"),
      ),
      "Snapshot authentication failed",
    );
    const result = await command({
      executable: this.options.age,
      args: ["-d", "-i", "/dev/fd/3"],
      input: Buffer.from(parsed.data.data, "base64"),
      identity: `${this.options.profile.identity}\n`,
    });
    check(result.code === 0, "Snapshot decryption failed");
    const envelope: Envelope = parseEnvelope(result.output.toString());
    check(
      envelope.group === this.options.profile.config.group,
      "Snapshot group mismatch",
    );
    return envelope;
  }
  envelope(snapshot: Snapshot): Envelope {
    return {
      group: this.options.profile.config.group,
      device: this.options.device,
      revision: randomUUID(),
      createdAt: new Date(
        Math.max(
          Date.now(),
          Date.parse(
            this.options.profile.trustedAfter ?? "1970-01-01T00:00:00Z",
          ) + 1,
        ),
      ).toISOString(),
      snapshot,
    };
  }
  pin(envelope: Envelope): void {
    check(
      !this.options.profile.trustedAfter ||
        envelope.createdAt >= this.options.profile.trustedAfter,
      "Older remote snapshot rejected; use explicit history restore",
    );
    this.options.profile.trustedAfter = envelope.createdAt;
    saveProfile(this.options.profile);
  }
  async part(key: string, offset: number): Promise<Part | null> {
    const value: unknown = await this.call(
      `async () => { try { const r=await cloudflare.request({method:"GET",path:${JSON.stringify(this.path(key))}}); if(r.status!==200) throw new Error("Unexpected object response"); const v=r.result; if(v?.version!==2 || typeof v.data!=="string" || typeof v.mac!=="string") throw new Error("Invalid object format"); return {total:v.data.length,mac:v.mac,part:v.data.slice(${offset},${offset + PART})}; } catch(e) { throw new Error("Object retrieval failed"); } }`,
    );
    return partSchema.parse(value);
  }
  async listed(prefix: string): Promise<string[]> {
    const config = this.options.profile.config;
    const value = listSchema.parse(
      await this.call(
        `async () => { const r=await cloudflare.request({method:"GET",path:${JSON.stringify(`/accounts/${config.accountId}/r2/buckets/${config.bucket}/objects`)},query:{prefix:${JSON.stringify(`v1/${config.group}/${prefix}`)},per_page:1000}}); if(!r.success || r.status!==200 || !Array.isArray(r.result)) throw new Error("Object listing failed"); return {items:r.result.map(v=>({key:v.key})),truncated:!!r.result_info?.cursor || !!r.result_info?.is_truncated || r.result.length>=1000}; }`,
      ),
    );
    return value.items.map((item) =>
      item.key.slice(`v1/${config.group}/`.length),
    );
  }
  async read(key: string): Promise<RemoteValue | null> {
    this.path(key);
    // An explicit successful exact-key listing is required to recognize absence.
    if (!(await this.listed(key)).some((item) => item === key)) {
      check(
        key !== "latest.age" || this.options.profile.trustedAfter === null,
        "Previously trusted latest snapshot is missing",
      );
      return null;
    }
    const first: Part | null = await this.part(key, 0);
    check(first, "Object missing");
    const pieces: string[] = [first.part];
    for (const offset of Array.from(
      { length: Math.ceil(first.total / PART) - 1 },
      (_, index) => (index + 1) * PART,
    )) {
      const next: Part | null = await this.part(key, offset);
      check(
        next && next.mac === first.mac && next.total === first.total,
        "Remote object changed during download",
      );
      pieces.push(next.part);
    }
    const data: string = pieces.join("");
    check(data.length === first.total, "Truncated ciphertext");
    const envelope: Envelope = await this.decrypt(
      Buffer.from(JSON.stringify({ version: 2, data, mac: first.mac })),
    );
    if (key === "latest.age") this.pin(envelope);
    return { tag: first.mac, envelope };
  }
  async put(key: string, bytes: Buffer): Promise<string> {
    const document = signedSchema.parse(JSON.parse(bytes.toString()));
    check(bytes.length <= 100000, "Upload exceeds transport limit");
    const result = z
      .object({ success: z.literal(true), status: z.literal(200) })
      .safeParse(
        await this.call(
          `async () => { const r=await cloudflare.request({method:"PUT",path:${JSON.stringify(this.path(key))},contentType:"application/json",body:{success:true,result:${JSON.stringify(document)},errors:[],messages:[]}}); return {success:r.success,status:r.status}; }`,
        ),
      );
    check(result.success, "R2 upload failed");
    return document.mac;
  }
  async publish(snapshot: Snapshot): Promise<string> {
    const envelope: Envelope = this.envelope(snapshot);
    const bytes: Buffer = await this.encrypt(envelope);
    await this.put(
      `history/${envelope.createdAt.replaceAll(":", "-")}-${envelope.device}-${envelope.revision}.age`,
      bytes,
    );
    const tag: string = await this.put("latest.age", bytes);
    this.pin(envelope);
    return tag;
  }
  async history(): Promise<string[]> {
    return this.listed("history/");
  }
}
