import { AddDialogSession } from "./addDialog";
let n = 0;
const check = (c: boolean, m: string) => {
  n++;
  if (!c) throw new Error("FAIL: " + m);
};
const calls: string[] = [];
let tick: (() => void) | null = null;
const timers = {
  setInterval: (f: () => void) => {
    tick = f;
    return 1;
  },
  clearInterval: () => {
    tick = null;
  },
};
const api = {
  addDialogHeartbeat: async (id: string) => {
    calls.push(`hb:${id}`);
  },
  addDialogFinish: (id: string, o?: { beacon?: boolean }) => {
    calls.push(`fin:${id}:${o?.beacon ? "beacon" : "fetch"}`);
  },
};
const s = new AddDialogSession(api, timers);
const id1 = s.id;
s.noteAdded({ held: false });
check(!s.held && tick === null, "unheld add starts nothing");
check(s.finish() === false && calls.length === 0, "nothing held: no finish call");
s.noteAdded({ held: true });
check(s.held && tick !== null, "held add starts heartbeat");
tick!();
check(calls[0] === `hb:${id1}`, "heartbeat uses dialog id");
s.noteAdded({ held: true });
check(s.finish() === true, "finish reports held");
check(calls[1] === `fin:${id1}:fetch` && tick === null, "finish sent once, heartbeat stopped");
check(s.finish(true) === false && calls.length === 2, "second finish (unmount/pagehide) is a no-op");
check(s.id !== id1, "new id for the next use");
s.noteAdded({ held: true });
s.finish(true);
check(calls[2].endsWith(":beacon"), "tab close uses the beacon");
console.log(`addDialog: ${n}/${n} checks passed`);
