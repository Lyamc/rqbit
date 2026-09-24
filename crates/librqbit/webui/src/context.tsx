import { createContext } from "react";
import { LimitsConfig, RqbitAPI, SessionStats } from "./api-types";

export const APIContext = createContext<RqbitAPI>({
  listTorrents: () => {
    throw new Error("Function not implemented.");
  },
  getTorrentDetails: () => {
    throw new Error("Function not implemented.");
  },
  getTorrentStats: () => {
    throw new Error("Function not implemented.");
  },
  getPeerStats: () => {
    throw new Error("Function not implemented.");
  },
  uploadTorrent: () => {
    throw new Error("Function not implemented.");
  },
  uploadTorrentFromServerPath: () => {
    throw new Error("Function not implemented.");
  },
  fsRoots: () => {
    throw new Error("Function not implemented.");
  },
  fsList: () => {
    throw new Error("Function not implemented.");
  },
  extractUpload: () => {
    throw new Error("Function not implemented.");
  },
  updateOnlyFiles: () => {
    throw new Error("Function not implemented.");
  },
  pause: () => {
    throw new Error("Function not implemented.");
  },
  start: () => {
    throw new Error("Function not implemented.");
  },
  restart: () => {
    throw new Error("Function not implemented.");
  },
  fixErrors: () => {
    throw new Error("Function not implemented.");
  },
  forget: () => {
    throw new Error("Function not implemented.");
  },
  delete: () => {
    throw new Error("Function not implemented.");
  },
  getTorrentStreamUrl: () => {
    throw new Error("Function not implemented.");
  },
  getStreamLogsUrl: function (): string | null {
    throw new Error("Function not implemented.");
  },
  getPlaylistUrl: function (index: number): string | null {
    throw new Error("Function not implemented.");
  },
  stats: function (): Promise<SessionStats> {
    throw new Error("Function not implemented.");
  },
  getTorrentHaves: function (index: number): Promise<Uint8Array> {
    throw new Error("Function not implemented.");
  },
  getLimits: function (): Promise<LimitsConfig> {
    throw new Error("Function not implemented.");
  },
  setLimits: function (limits: LimitsConfig): Promise<void> {
    throw new Error("Function not implemented.");
  },
  getPreferences: function () {
    throw new Error("Function not implemented.");
  },
  renameFile: function () {
    return Promise.reject(new Error("API not set"));
  },
  relocateTorrent: function () {
    return Promise.reject(new Error("API not set"));
  },
  setPreferences: function () {
    throw new Error("Function not implemented.");
  },
  getAdminStatus: function () {
    throw new Error("Function not implemented.");
  },
  updateAdminConfig: function () {
    throw new Error("Function not implemented.");
  },
  reloadPreferences: function () {
    throw new Error("Function not implemented.");
  },
  restartProcess: function () {
    throw new Error("Function not implemented.");
  },
});
