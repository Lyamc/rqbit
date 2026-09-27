import React, { useCallback, useContext, useEffect, useState } from "react";
import { APIContext } from "../../context";
import {
  AdminConfigUpdate,
  AdminStatus,
  ErrorDetails,
} from "../../api-types";
import { Fieldset } from "../forms/Fieldset";
import { FormCheckbox } from "../forms/FormCheckbox";
import { FormInput } from "../forms/FormInput";
import { Spinner } from "../Spinner";


function errText(e: unknown): string {
  const d = e as ErrorDetails;
  if (typeof d?.text === "string" && d.text) return d.text;
  if (e instanceof Error) return e.message;
  return String(e);
}

export const AdminTab: React.FC = () => {
  const API = useContext(APIContext);
  const [status, setStatus] = useState<AdminStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);

  const [listenAddr, setListenAddr] = useState("");
  const [authEnabled, setAuthEnabled] = useState(false);
  const [authUser, setAuthUser] = useState("");
  const [authPassword, setAuthPassword] = useState("");

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const s = await API.getAdminStatus();
      setStatus(s);
      setListenAddr(s.persisted.http_api_listen_addr ?? "");
      setAuthEnabled(s.persisted.basic_auth_enabled);
      setAuthUser(s.persisted.basic_auth_user ?? "");
      setAuthPassword("");
    } catch (e) {
      setError(errText(e));
    } finally {
      setLoading(false);
    }
  }, [API]);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const saveAdmin = async () => {
    setBusy(true);
    setError(null);
    setMessage(null);
    try {
      const patch: AdminConfigUpdate = {
        http_api_listen_addr: listenAddr.trim() || null,
        basic_auth_enabled: authEnabled,
      };
      if (authEnabled) {
        patch.basic_auth_user = authUser.trim();
        if (authPassword) {
          patch.basic_auth_password = authPassword;
        }
      }
      await API.updateAdminConfig(patch);
      setMessage(
        "Saved to admin.json. Listen address and auth apply after process restart (env overrides file).",
      );
      setAuthPassword("");
      await refresh();
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  };

  const reloadPrefs = async () => {
    setBusy(true);
    setError(null);
    setMessage(null);
    try {
      await API.reloadPreferences();
      setMessage("Reloaded preferences.json from disk into the live session.");
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  };

  const restart = async () => {
    if (
      !window.confirm(
        "Restart the rqbit process? Active torrents will resume after systemd brings the service back (exit code 75 / Restart=on-failure). Continue?",
      )
    ) {
      return;
    }
    setBusy(true);
    setError(null);
    setMessage(null);
    try {
      await API.restartProcess();
      setMessage(
        "Restart signaled. The UI may disconnect briefly while the process exits and systemd restarts it.",
      );
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  };

  if (loading) {
    return (
      <div className="flex justify-center p-6">
        <Spinner />
      </div>
    );
  }

  return (
    <div className="text-secondary py-2 space-y-4">
      {error && (
        <div className="border border-error/40 bg-error/10 text-error rounded p-2 text-sm">
          {error}
        </div>
      )}
      {message && (
        <div className="border border-primary/30 bg-primary/10 text-text rounded p-2 text-sm">
          {message}
        </div>
      )}

      <Fieldset label="How settings persist">
        <ul className="text-sm space-y-1 list-disc pl-5">
          <li>
            <strong className="text-text">Live</strong> ? Rate limits,
            downloads, organize, and completion actions save immediately to{" "}
            <code className="bg-surface-sunken px-1 rounded">preferences.json</code>{" "}
            /{" "}
            <code className="bg-surface-sunken px-1 rounded">limits.json</code>.
          </li>
          <li>
            <strong className="text-text">Restart required</strong> ? HTTP listen
            address and basic auth are written to{" "}
            <code className="bg-surface-sunken px-1 rounded">admin.json</code>.
            Environment variables (e.g. NixOS systemd unit) override the file.
          </li>
        </ul>
        {status?.notes?.map((n, i) => (
          <p key={i} className="text-sm text-tertiary mt-2">
            {n}
          </p>
        ))}
      </Fieldset>

      <Fieldset label="Runtime status">
        <div className="text-sm space-y-1 font-mono">
          <div>version: {status?.version}</div>
          <div>prefs: {status?.preferences_path}</div>
          <div>admin: {status?.admin_path}</div>
          <div>
            env listen: {status?.env_http_listen_addr || "(unset)"}
            {status?.env_http_listen_addr
              ? " ? overrides admin.json"
              : ""}
          </div>
          <div>
            env basic auth: {status?.env_basic_auth_set ? "set" : "unset"}
          </div>
          <div>
            restart API:{" "}
            {status?.restart_supported ? "available" : "not available"}
          </div>
        </div>
      </Fieldset>

      <Fieldset label="HTTP API (restart required)">
        <FormInput
          name="http_api_listen_addr"
          label="Listen address"
          value={listenAddr}
          placeholder="0.0.0.0:9030"
          help="Host:port for the HTTP API / web UI. Applied on next start if RQBIT_HTTP_API_LISTEN_ADDR is unset."
          onChange={(e) => setListenAddr(e.target.value)}
        />
        <FormCheckbox
          checked={authEnabled}
          name="basic_auth_enabled"
          label="Enable HTTP basic authentication"
          help="Simple username/password for the API and web UI. Applied on next start if RQBIT_HTTP_BASIC_AUTH_USERPASS is unset."
          onChange={(e) => setAuthEnabled(e.target.checked)}
        />
        {authEnabled && (
          <>
            <FormInput
              name="basic_auth_user"
              label="Username"
              value={authUser}
              onChange={(e) => setAuthUser(e.target.value)}
            />
            <FormInput
              name="basic_auth_password"
              label={
                status?.persisted.basic_auth_password_set
                  ? "Password (leave blank to keep current)"
                  : "Password"
              }
              value={authPassword}
              inputType="password"
              onChange={(e) => setAuthPassword(e.target.value)}
            />
          </>
        )}
        <button
          type="button"
          disabled={busy}
          onClick={saveAdmin}
          className="px-3 py-1.5 border border-divider rounded hover:bg-surface-raised disabled:opacity-50"
        >
          Save admin.json
        </button>
      </Fieldset>

      <Fieldset label="Operations">
        <div className="flex flex-wrap gap-2">
          <button
            type="button"
            disabled={busy}
            onClick={reloadPrefs}
            className="px-3 py-1.5 border border-divider rounded hover:bg-surface-raised disabled:opacity-50"
          >
            Force-reload preferences.json
          </button>
          <button
            type="button"
            disabled={busy || !status?.restart_supported}
            onClick={restart}
            className="px-3 py-1.5 border border-error/50 text-error rounded hover:bg-surface-raised disabled:opacity-50"
          >
            Restart rqbit process
          </button>
        </div>
        <p className="text-sm text-tertiary mt-2">
          Restart asks the process to exit with code 75. On NixOS with{" "}
          <code className="bg-surface-sunken px-1 rounded">Restart=on-failure</code>
          , systemd brings it back. This is not a full{" "}
          <code className="bg-surface-sunken px-1 rounded">nixos-rebuild</code> ?
          changing the unit still requires a rebuild.
        </p>
      </Fieldset>
    </div>
  );
};
