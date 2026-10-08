/**
 * Keeps the torrent list current over the server's `GET /stream/torrents`:
 * a WebSocket (snapshot, then deltas) when it connects, else `?since=` delta polling
 * on the same URL. Reconnects with backoff, resyncs on gaps, and keeps retrying the
 * WebSocket while polling. The rest of the UI only sees `onTorrents(list)`.
 */
import { TorrentListItem } from "../api-types";
import { FeedClient, FeedMessage } from "./listFeed";

export type FeedTransport = "connecting" | "websocket" | "polling";

export interface FeedEnv {
  /** ws:// or wss:// URL of `/stream/torrents`, or null for polling only. */
  wsUrl: string | null;
  /** `GET /stream/torrents?since=..&epoch=..` (null: snapshot). */
  poll: (since: { seq: number; epoch: string } | null) => Promise<FeedMessage>;
  WebSocket?: { new (url: string): WebSocketLike } | null;
  /** Raw-deflate decoding (`DecompressionStream`); null sends plain JSON frames. */
  makeInflater?:
    | ((
        onLine: (line: string) => void,
        onError: (e: unknown) => void,
      ) => Inflater)
    | null;
  setTimeout: (f: () => void, ms: number) => unknown;
  clearTimeout: (t: unknown) => void;
  now: () => number;
}

export interface WebSocketLike {
  binaryType: string;
  readyState: number;
  onopen: ((ev: any) => void) | null;
  onmessage: ((ev: { data: any }) => void) | null;
  onclose: ((ev: any) => void) | null;
  onerror: ((ev: any) => void) | null;
  send(data: string): void;
  close(): void;
}

export interface Inflater {
  write(chunk: Uint8Array): void;
  close(): void;
}

export interface FeedCallbacks {
  onTorrents: (torrents: TorrentListItem[]) => void;
  /** An error to show (null clears it): only when no transport works. */
  onError: (e: unknown | null) => void;
  onTransport?: (t: FeedTransport) => void;
}

/** Silence after which a socket is considered dead (server heartbeats every 15 s). */
export const WATCHDOG_MS = 40_000;
/** Failed WebSocket attempts in a row before polling takes over. */
export const WS_FAILURES_BEFORE_POLLING = 2;
const RECONNECT_MIN_MS = 1_000;
const RECONNECT_MAX_MS = 30_000;
const WS_RETRY_WHILE_POLLING_MIN_MS = 10_000;
const WS_RETRY_WHILE_POLLING_MAX_MS = 60_000;
const POLL_ERROR_RETRY_MS = 5_000;

export class TorrentFeed {
  private client = new FeedClient();
  private ws: WebSocketLike | null = null;
  private inflater: Inflater | null = null;
  private wsOpen = false;
  private wsFailures = 0;
  private reconnectMs = RECONNECT_MIN_MS;
  private wsRetryMs = WS_RETRY_WHILE_POLLING_MIN_MS;
  private reconnectTimer: unknown = null;
  private watchdog: unknown = null;
  private pollTimer: unknown = null;
  private polling = false;
  private pollInFlight = false;
  private stopped = true;
  private tickMs = 1000;
  transport: FeedTransport = "connecting";

  constructor(
    private env: FeedEnv,
    private cb: FeedCallbacks,
  ) {}

  start() {
    this.stopped = false;
    if (this.canUseWs()) this.connect();
    else this.startPolling();
  }

  stop() {
    this.stopped = true;
    this.closeWs();
    this.env.clearTimeout(this.reconnectTimer);
    this.env.clearTimeout(this.pollTimer);
    this.polling = false;
  }

  /** Update interval wanted by the UI (1 s while something is active, else 5 s). */
  setTickMs(ms: number) {
    if (ms === this.tickMs) return;
    this.tickMs = ms;
    if (this.wsOpen) this.send({ type: "tick", ms });
  }

  /** Get changes now (after an action). */
  refresh() {
    if (this.wsOpen) this.send({ type: "refresh" });
    else if (this.polling) this.pollNow();
  }

  private canUseWs() {
    return !!(this.env.wsUrl && this.env.WebSocket);
  }

  private setTransport(t: FeedTransport) {
    if (this.transport !== t) {
      this.transport = t;
      this.cb.onTransport?.(t);
    }
  }

  // ---- WebSocket ----

  private connect() {
    if (this.stopped) return;
    const deflate = !!this.env.makeInflater;
    const url = `${this.env.wsUrl}?tick_ms=${this.tickMs}${deflate ? "&enc=deflate" : ""}`;
    let ws: WebSocketLike;
    try {
      ws = new this.env.WebSocket!(url);
    } catch (e) {
      this.onWsClosed(false);
      return;
    }
    this.ws = ws;
    this.wsOpen = false;
    ws.binaryType = "arraybuffer";
    if (deflate) {
      this.inflater = this.env.makeInflater!(
        (line) => this.onMessage(line),
        () => ws.close(),
      );
    }
    ws.onopen = () => {
      if (ws !== this.ws) return;
      this.wsOpen = true;
      this.wsFailures = 0;
      this.reconnectMs = RECONNECT_MIN_MS;
      this.wsRetryMs = WS_RETRY_WHILE_POLLING_MIN_MS;
      this.armWatchdog();
    };
    ws.onmessage = (ev) => {
      if (ws !== this.ws) return;
      this.armWatchdog();
      if (typeof ev.data === "string") this.onMessage(ev.data);
      else if (this.inflater) this.inflater.write(new Uint8Array(ev.data));
    };
    ws.onerror = () => {};
    ws.onclose = () => {
      if (ws !== this.ws) return;
      this.onWsClosed(this.wsOpen);
    };
  }

  private onMessage(text: string) {
    let msg: FeedMessage;
    try {
      msg = JSON.parse(text);
    } catch {
      return;
    }
    const r = this.client.apply(msg);
    if (r !== "ok") {
      this.send({ type: "resync" });
      return;
    }
    if (msg.type === "snapshot" || msg.type === "delta") {
      if (this.polling) this.stopPolling();
      this.setTransport("websocket");
      this.cb.onError(null);
      this.cb.onTorrents(this.client.torrents());
    }
  }

  private send(v: unknown) {
    try {
      this.ws?.send(JSON.stringify(v));
    } catch {
      // closed; onclose handles it
    }
  }

  private armWatchdog() {
    this.env.clearTimeout(this.watchdog);
    this.watchdog = this.env.setTimeout(() => this.ws?.close(), WATCHDOG_MS);
  }

  private closeWs() {
    this.env.clearTimeout(this.watchdog);
    const ws = this.ws;
    this.ws = null;
    this.wsOpen = false;
    this.inflater?.close();
    this.inflater = null;
    if (ws) {
      ws.onopen = ws.onmessage = ws.onclose = ws.onerror = null;
      try {
        ws.close();
      } catch {
        // already closed
      }
    }
  }

  private onWsClosed(wasOpen: boolean) {
    this.closeWs();
    if (this.stopped) return;
    if (!wasOpen) this.wsFailures++;
    if (this.wsFailures >= WS_FAILURES_BEFORE_POLLING) {
      // The WebSocket doesn't get through (proxy, network): poll, and retry now and then.
      if (!this.polling) this.startPolling();
      this.scheduleReconnect(this.wsRetryMs);
      this.wsRetryMs = Math.min(
        this.wsRetryMs * 2,
        WS_RETRY_WHILE_POLLING_MAX_MS,
      );
    } else {
      // Keep the list fresh while reconnecting.
      if (this.client.hasState && !this.polling) this.pollOnce();
      this.scheduleReconnect(this.reconnectMs);
      this.reconnectMs = Math.min(this.reconnectMs * 2, RECONNECT_MAX_MS);
    }
  }

  private scheduleReconnect(ms: number) {
    this.env.clearTimeout(this.reconnectTimer);
    this.reconnectTimer = this.env.setTimeout(() => this.connect(), ms);
  }

  // ---- polling ----

  private startPolling() {
    this.polling = true;
    this.pollNow();
  }

  private stopPolling() {
    this.polling = false;
    this.env.clearTimeout(this.pollTimer);
  }

  private pollNow() {
    this.env.clearTimeout(this.pollTimer);
    void this.pollLoopStep();
  }

  private async pollOnce() {
    try {
      await this.pollAndApply();
    } catch {
      // the reconnect logic reports errors
    }
  }

  private async pollAndApply() {
    const c = this.client;
    let msg = await this.env.poll(
      c.hasState ? { seq: c.seq, epoch: c.epoch! } : null,
    );
    if (c.apply(msg) !== "ok") {
      c.reset();
      msg = await this.env.poll(null);
      if (c.apply(msg) !== "ok")
        throw new Error("bad torrent list from server");
    }
    if (this.stopped) return;
    if (!this.wsOpen) this.setTransport("polling");
    this.cb.onError(null);
    this.cb.onTorrents(c.torrents());
  }

  private async pollLoopStep() {
    if (!this.polling || this.stopped || this.pollInFlight) return;
    this.pollInFlight = true;
    let delay = this.tickMs;
    try {
      await this.pollAndApply();
    } catch (e) {
      if (!this.wsOpen) this.cb.onError(e);
      delay = POLL_ERROR_RETRY_MS;
    } finally {
      this.pollInFlight = false;
    }
    if (this.polling && !this.stopped) {
      this.env.clearTimeout(this.pollTimer);
      this.pollTimer = this.env.setTimeout(
        () => void this.pollLoopStep(),
        delay,
      );
    }
  }
}

/** `DecompressionStream("deflate-raw")` over the socket's binary frames, split into
 *  JSON lines. Null when the browser can't do it (then the server sends text). */
export function browserInflater():
  | ((
      onLine: (line: string) => void,
      onError: (e: unknown) => void,
    ) => Inflater)
  | null {
  const DS = (globalThis as any).DecompressionStream;
  if (typeof DS !== "function") return null;
  try {
    new DS("deflate-raw");
  } catch {
    return null;
  }
  return (onLine, onError) => {
    const ds = new DS("deflate-raw");
    const writer = ds.writable.getWriter();
    const reader = ds.readable.getReader();
    const td = new TextDecoder();
    let buf = "";
    let closed = false;
    (async () => {
      try {
        for (;;) {
          const { value, done } = await reader.read();
          if (done || closed) return;
          buf += td.decode(value, { stream: true });
          let i: number;
          while ((i = buf.indexOf("\n")) >= 0) {
            const line = buf.slice(0, i);
            buf = buf.slice(i + 1);
            if (line) onLine(line);
          }
        }
      } catch (e) {
        if (!closed) onError(e);
      }
    })();
    return {
      write: (chunk) => {
        writer.write(chunk).catch((e: unknown) => !closed && onError(e));
      },
      close: () => {
        closed = true;
        writer.abort().catch(() => {});
        reader.cancel().catch(() => {});
      },
    };
  };
}
