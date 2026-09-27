/** "Start after I finish the Add dialog" (client side).
 *
 * Every add from the Add window carries this dialog's id (`add_dialog_id`). When
 * the server preference is on, it adds those torrents paused and answers
 * `held: true`. From the first held add on, the window sends a heartbeat every
 * 15 s, and when it goes away (Add finished and auto-closed, Cancel/×, Escape,
 * unmount) it calls `finish`, which starts every held torrent still there. On a
 * tab/window close the finish goes out with `navigator.sendBeacon`; if even that
 * is lost, the server starts them once heartbeats stop (after about 2.5 min). */

export interface AddDialogApi {
  addDialogHeartbeat?: (dialogId: string) => Promise<unknown>;
  addDialogFinish?: (dialogId: string, opts?: { beacon?: boolean }) => unknown;
}

export interface Timers {
  setInterval: (f: () => void, ms: number) => unknown;
  clearInterval: (h: unknown) => void;
}

export const HEARTBEAT_MS = 15_000;

export const newDialogId = () =>
  `dlg-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;

export class AddDialogSession {
  id = newDialogId();
  held = false;
  private timer: unknown = undefined;

  constructor(
    private api: AddDialogApi,
    private timers: Timers = {
      setInterval: (f, ms) => window.setInterval(f, ms),
      clearInterval: (h) => window.clearInterval(h as number),
    },
  ) {}

  /** Call with each add response. */
  noteAdded(res: { held?: boolean } | undefined) {
    if (!res?.held || this.held) return;
    this.held = true;
    const id = this.id;
    this.timer = this.timers.setInterval(() => {
      const p = this.api.addDialogHeartbeat?.(id);
      if (p && typeof (p as Promise<unknown>).catch === "function") {
        (p as Promise<unknown>).catch(() => {});
      }
    }, HEARTBEAT_MS);
  }

  /** The dialog went away: start what it held. Safe to call more than once. */
  finish(beacon = false): boolean {
    if (this.timer !== undefined) {
      this.timers.clearInterval(this.timer);
      this.timer = undefined;
    }
    if (!this.held) return false;
    this.held = false;
    try {
      const p = this.api.addDialogFinish?.(this.id, { beacon });
      if (p && typeof (p as Promise<unknown>).catch === "function") {
        (p as Promise<unknown>).catch(() => {});
      }
    } catch {
      // best effort; the server times the dialog out anyway
    }
    this.id = newDialogId();
    return true;
  }
}
