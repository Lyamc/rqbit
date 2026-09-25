import {
  AddTorrentResponse,
  ErrorDetails,
  LimitsConfig,
  SessionPreferences,
  AdminStatus,
  AdminConfigUpdate,
  AdminConfigPublic,
  ListTorrentsResponse,
  PeerStatsSnapshot,
  RqbitAPI,
  RequestOptions,
  SessionStats,
  TorrentDetails,
  TorrentStats,
  FsRootsResponse,
  FsListResponse,
  ExtractResponse,
} from "./api-types";

// Define API URL and base path
const apiUrl = (() => {
  if (window.origin === "null") {
    return "http://localhost:3030";
  }

  const url = new URL(window.location.href);

  // assume Vite devserver
  if (url.port == "3031" || url.port == "1420") {
    return `${url.protocol}//${url.hostname}:3030`;
  }

  // Remove "/web" or "/web/" from the end and also ending slash.
  const path = /(.*?)\/?(\/web\/?)?$/.exec(url.pathname)![1] ?? "";
  return path;
})();

const makeBinaryRequest = async (path: string): Promise<ArrayBuffer> => {
  const url = apiUrl + path;
  const response = await fetch(url, {
    method: "GET",
    headers: {
      Accept: "application/octet-stream",
    },
  });

  if (!response.ok) {
    throw new Error(`HTTP ${response.status}: ${response.statusText}`);
  }

  return response.arrayBuffer();
};

const makeRequest = async (
  method: string,
  path: string,
  data?: any,
  isJson?: boolean,
  init?: RequestOptions,
): Promise<any> => {
  console.log(method, path);
  const url = apiUrl + path;
  let options: RequestInit = {
    method,
    headers: {
      Accept: "application/json",
    },
    signal: init?.signal,
  };
  if (isJson) {
    options.headers = {
      Accept: "application/json",
      "Content-Type": "application/json",
    };
    options.body = JSON.stringify(data);
  } else {
    options.body = data;
  }

  let error: ErrorDetails = {
    method: method,
    path: path,
    text: "",
  };

  let response: Response;

  try {
    response = await fetch(url, options);
  } catch (e) {
    error.text =
      init?.signal?.aborted || (e as Error)?.name === "AbortError"
        ? "request aborted"
        : "network error";
    return Promise.reject(error);
  }

  error.status = response.status;
  error.statusText = `${response.status} ${response.statusText}`;

  if (!response.ok) {
    const errorBody = await response.text();
    try {
      const json = JSON.parse(errorBody);
      error.text =
        json.human_readable !== undefined
          ? json.human_readable
          : JSON.stringify(json, null, 2);
    } catch (e) {
      error.text = errorBody;
    }
    return Promise.reject(error);
  }
  const result = await response.json();
  return result;
};

export const API: RqbitAPI & { getVersion: () => Promise<string> } = {
  getStreamLogsUrl: () => apiUrl + "/stream_logs",
  listTorrents: (opts?: {
    withStats?: boolean;
  }): Promise<ListTorrentsResponse> => {
    const url = opts?.withStats ? "/torrents?with_stats=true" : "/torrents";
    return makeRequest("GET", url);
  },
  getTorrentDetails: (index: number): Promise<TorrentDetails> => {
    return makeRequest("GET", `/torrents/${index}`);
  },
  getTorrentStats: (index: number): Promise<TorrentStats> => {
    return makeRequest("GET", `/torrents/${index}/stats/v1`);
  },
  getPeerStats: (index: number): Promise<PeerStatsSnapshot> => {
    return makeRequest("GET", `/torrents/${index}/peer_stats?state=live`);
  },
  stats: (): Promise<SessionStats> => {
    return makeRequest("GET", "/stats");
  },

  uploadTorrent: (data, opts, init): Promise<AddTorrentResponse> => {
    // Prefer caller overwrite; default true to match prior webui behavior.
    const overwrite = opts?.overwrite ?? true;
    let url = `/torrents?overwrite=${overwrite}`;
    if (opts?.list_only) {
      url += "&list_only=true";
    }
    if (opts?.only_files != null) {
      url += `&only_files=${opts.only_files.join(",")}`;
    }
    if (opts?.peer_opts?.connect_timeout) {
      url += `&peer_connect_timeout=${opts.peer_opts.connect_timeout}`;
    }
    if (opts?.peer_opts?.read_write_timeout) {
      url += `&peer_read_write_timeout=${opts.peer_opts.read_write_timeout}`;
    }
    if (opts?.initial_peers) {
      url += `&initial_peers=${opts.initial_peers.join(",")}`;
    }
    if (opts?.output_folder) {
      url += `&output_folder=${encodeURIComponent(opts.output_folder)}`;
    }
    if (opts?.adopt_foreign_incomplete) {
      url += `&adopt_foreign_incomplete=${opts.adopt_foreign_incomplete}`;
    }
    if (typeof data === "string") {
      url += "&is_url=true";
    }
    return makeRequest("POST", url, data, false, init);
  },

  uploadTorrentFromServerPath: (
    path,
    opts,
    init,
  ): Promise<AddTorrentResponse> => {
    const overwrite = opts?.overwrite ?? true;
    let url = `/torrents?overwrite=${overwrite}&from_server_path=${encodeURIComponent(path)}`;
    if (opts?.list_only) {
      url += "&list_only=true";
    }
    if (opts?.output_folder) {
      url += `&output_folder=${encodeURIComponent(opts.output_folder)}`;
    }
    if (opts?.adopt_foreign_incomplete) {
      url += `&adopt_foreign_incomplete=${opts.adopt_foreign_incomplete}`;
    }
    return makeRequest("POST", url, "", false, init);
  },

  fsRoots: (): Promise<FsRootsResponse> => {
    return makeRequest("GET", "/fs/roots");
  },

  fsList: (path, opts): Promise<FsListResponse> => {
    let url = `/fs/list?path=${encodeURIComponent(path)}`;
    if (opts?.recursive) url += "&recursive=true";
    if (opts?.torrentsOnly) url += "&torrents_only=true";
    return makeRequest("GET", url);
  },

  extractUpload: async (data): Promise<ExtractResponse> => {
    const url = apiUrl + "/fs/extract";
    let error: ErrorDetails = {
      method: "POST",
      path: "/fs/extract",
      text: "",
    };
    let response: Response;
    try {
      response = await fetch(url, {
        method: "POST",
        headers: { Accept: "application/json" },
        body: data,
      });
    } catch (e) {
      error.text = "network error";
      return Promise.reject(error);
    }
    error.status = response.status;
    error.statusText = `${response.status} ${response.statusText}`;
    if (!response.ok) {
      const errorBody = await response.text();
      try {
        const json = JSON.parse(errorBody);
        error.text =
          json.human_readable !== undefined
            ? json.human_readable
            : JSON.stringify(json, null, 2);
      } catch {
        error.text = errorBody;
      }
      return Promise.reject(error);
    }
    return response.json();
  },

  updateOnlyFiles: (index: number, files: number[]): Promise<void> => {
    let url = `/torrents/${index}/update_only_files`;
    return makeRequest(
      "POST",
      url,
      {
        only_files: files,
      },
      true,
    );
  },

  pause: (index: number): Promise<void> => {
    return makeRequest("POST", `/torrents/${index}/pause`);
  },

  start: (index: number): Promise<void> => {
    return makeRequest("POST", `/torrents/${index}/start`);
  },

  restart: (index: number): Promise<void> => {
    return makeRequest("POST", `/torrents/${index}/restart`);
  },

  fixErrors: (index: number): Promise<void> => {
    return makeRequest("POST", `/torrents/${index}/fix_errors`);
  },

  forget: (index: number): Promise<void> => {
    return makeRequest("POST", `/torrents/${index}/forget`);
  },

  delete: (index: number): Promise<void> => {
    return makeRequest("POST", `/torrents/${index}/delete`);
  },
  getVersion: async (): Promise<string> => {
    const r = await makeRequest("GET", "/");
    return r.version;
  },
  getTorrentStreamUrl: (
    index: number,
    file_id: number,
    filename?: string | null,
  ) => {
    let url = apiUrl + `/torrents/${index}/stream/${file_id}`;
    if (!!filename) {
      url += `/${filename}`;
    }
    return url;
  },
  getPlaylistUrl: (index: number) => {
    return (apiUrl || window.origin) + `/torrents/${index}/playlist`;
  },
  getTorrentHaves: async (index: number): Promise<Uint8Array> => {
    return new Uint8Array(await makeBinaryRequest(`/torrents/${index}/haves`));
  },
  getLimits: (): Promise<LimitsConfig> => {
    return makeRequest("GET", "/torrents/limits");
  },
  setLimits: (limits: LimitsConfig): Promise<void> => {
    return makeRequest("POST", "/torrents/limits", limits, true);
  },
  getPreferences: (): Promise<SessionPreferences> => {
    return makeRequest("GET", "/torrents/preferences");
  },
  renameFile: (
    index: number,
    fileId: number,
    newPath: string,
  ): Promise<void> => {
    return makeRequest("POST", `/torrents/${index}/rename_file`, {
      file_id: fileId,
      new_path: newPath,
    });
  },
  relocateTorrent: (
    index: number,
    destination: string,
    copy?: boolean,
  ): Promise<void> => {
    return makeRequest("POST", `/torrents/${index}/relocate`, {
      destination,
      copy: !!copy,
    });
  },
  setPreferences: (prefs: SessionPreferences): Promise<void> => {
    return makeRequest("POST", "/torrents/preferences", prefs, true);
  },
  getAdminStatus: (): Promise<AdminStatus> => {
    return makeRequest("GET", "/admin");
  },
  updateAdminConfig: (patch: AdminConfigUpdate): Promise<AdminConfigPublic> => {
    return makeRequest("POST", "/admin/config", patch, true);
  },
  reloadPreferences: (): Promise<SessionPreferences> => {
    return makeRequest("POST", "/admin/reload");
  },
  restartProcess: (): Promise<void> => {
    return makeRequest("POST", "/admin/restart");
  },
};
