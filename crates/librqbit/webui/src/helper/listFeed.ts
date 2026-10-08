/**
 * Client side of the server's lean torrent-list feed (`GET /stream/torrents`, see
 * `crates/librqbit/src/list_feed/mod.rs` for the format): applies snapshots and
 * delta messages, and turns lean rows back into the `TorrentListItem`s the rest of
 * the UI uses, exactly as `/torrents?with_stats=true` returns them (minus fields no
 * UI reads, which come back as 0).
 *
 * Pure functions; the transport (WebSocket / polling) is in torrentFeed.ts.
 */
import { TorrentListItem } from "../api-types";

export const FEED_PROTO = 1;

export type LeanRow = { id: number; [k: string]: unknown };

export interface FeedMessage {
  type: "snapshot" | "delta" | "heartbeat" | "pong";
  proto?: number;
  epoch?: string;
  seq?: number;
  base?: number;
  torrents?: LeanRow[];
  added?: LeanRow[];
  removed?: number[];
  changed?: LeanRow[];
  order?: number[];
}

/** RFC 7386 merge patch: null removes, objects merge recursively, the rest replaces. */
export function applyPatch(target: any, patch: any): any {
  if (patch === null || typeof patch !== "object" || Array.isArray(patch)) {
    return patch;
  }
  const out =
    target !== null && typeof target === "object" && !Array.isArray(target)
      ? { ...target }
      : {};
  for (const [k, v] of Object.entries(patch)) {
    if (v === null) delete out[k];
    else if (typeof v === "object" && !Array.isArray(v))
      out[k] = applyPatch(out[k], v);
    else out[k] = v;
  }
  return out;
}

export type ApplyResult = "ok" | "gap" | "invalid";

/** A client's copy of the feed: rows in the server's order, keyed by id. */
export class FeedClient {
  epoch: string | null = null;
  seq = 0;
  private rows = new Map<number, LeanRow>();
  private order: number[] = [];
  /** Expanded rows, rebuilt only for rows that changed. */
  private expanded = new Map<number, TorrentListItem>();

  get hasState(): boolean {
    return this.epoch !== null;
  }

  reset() {
    this.epoch = null;
    this.seq = 0;
    this.rows.clear();
    this.order = [];
    this.expanded.clear();
  }

  apply(msg: FeedMessage): ApplyResult {
    if (msg.type === "heartbeat" || msg.type === "pong") return "ok";
    if (
      msg.proto !== FEED_PROTO ||
      typeof msg.epoch !== "string" ||
      typeof msg.seq !== "number"
    )
      return "invalid";
    if (msg.type === "snapshot") {
      if (!Array.isArray(msg.torrents)) return "invalid";
      this.rows.clear();
      this.expanded.clear();
      this.order = [];
      for (const r of msg.torrents) {
        this.rows.set(r.id, r);
        this.order.push(r.id);
      }
    } else if (msg.type === "delta") {
      if (msg.epoch !== this.epoch || msg.base !== this.seq) return "gap";
      for (const p of msg.changed ?? []) {
        if (!this.rows.has(p.id)) return "gap";
      }
      const removed = new Set(msg.removed ?? []);
      for (const id of removed) {
        this.rows.delete(id);
        this.expanded.delete(id);
      }
      if (removed.size)
        this.order = this.order.filter((id) => !removed.has(id));
      for (const p of msg.changed ?? []) {
        this.rows.set(p.id, applyPatch(this.rows.get(p.id), p) as LeanRow);
        this.expanded.delete(p.id);
      }
      for (const r of msg.added ?? []) {
        if (!this.rows.has(r.id)) this.order.push(r.id);
        this.rows.set(r.id, r);
        this.expanded.delete(r.id);
      }
      if (msg.order) this.order = msg.order.filter((id) => this.rows.has(id));
    } else {
      return "invalid";
    }
    this.epoch = msg.epoch;
    this.seq = msg.seq;
    return "ok";
  }

  /** Lean rows in order (tests). */
  leanRows(): LeanRow[] {
    return this.order.map((id) => this.rows.get(id)!);
  }

  /** The list as `/torrents?with_stats=true` would return it. */
  torrents(): TorrentListItem[] {
    return this.order.map((id) => {
      let t = this.expanded.get(id);
      if (!t) {
        t = expandRow(this.rows.get(id)!);
        this.expanded.set(id, t);
      }
      return t;
    });
  }
}

const MIB = 1048576;

/** `{:.2} MiB/s` exactly as the server's Rust formatting does it (ties to even on
 *  the exact value), from integer bytes/s; `mbps` is `bps / 2^20` there too. */
export function formatSpeed(bps: number): string {
  const n = bps * 100; // hundredths of MiB/s, times 2^20 (exact below 2^53)
  let q = Math.floor(n / MIB);
  const r = n - q * MIB;
  if (r > MIB / 2 || (r === MIB / 2 && q % 2 === 1)) q += 1;
  const s = String(q).padStart(3, "0");
  return `${s.slice(0, -2)}.${s.slice(-2)} MiB/s`;
}

/** The server's `DurationWithHumanReadable` ("1h 2m", "3m 4s", "5s"). */
export function formatEta(ms: number): string {
  const total = Math.floor(ms / 1000);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  if (h > 0) return `${h}h ${m}m`;
  if (m > 0) return `${m}m ${s}s`;
  return `${s}s`;
}

const speed = (bps: unknown) => {
  const b = typeof bps === "number" ? bps : 0;
  return { mbps: b / MIB, human_readable: formatSpeed(b) };
};

/** Lean row → list item (fields no UI reads come back as 0). */
export function expandRow(row: LeanRow): TorrentListItem {
  const t: any = { ...row };
  if (!("name" in t)) t.name = null;
  if (t.stats) {
    const st: any = { ...t.stats };
    if (!("error" in st)) st.error = null;
    if (!("live" in st)) st.live = null;
    if (!("file_progress" in st)) st.file_progress = [];
    if (st.live) {
      const { download_bps, upload_bps, eta_ms, ...rest } = st.live as any;
      const live: any = { ...rest };
      const snap: any = { ...(rest.snapshot ?? {}) };
      snap.downloaded_and_checked_bytes ??= 0;
      snap.total_piece_download_ms ??= 0;
      const ps: any = { ...(snap.peer_stats ?? {}) };
      for (const k of [
        "queued",
        "connecting",
        "live",
        "live_tcp",
        "live_utp",
        "live_socks",
        "live_seeders",
        "seen",
        "dead",
        "not_needed",
        "steals",
      ])
        ps[k] ??= 0;
      snap.peer_stats = ps;
      live.snapshot = snap;
      live.average_piece_download_time ??= { secs: 0, nanos: 0 };
      live.download_speed = speed(download_bps);
      live.upload_speed = speed(upload_bps);
      live.time_remaining =
        typeof eta_ms === "number"
          ? {
              duration: {
                secs: Math.floor(eta_ms / 1000),
                nanos: (eta_ms % 1000) * 1_000_000,
              },
              human_readable: formatEta(eta_ms),
            }
          : null;
      st.live = live;
    }
    t.stats = st;
  }
  return t as TorrentListItem;
}
