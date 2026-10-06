import React from "react";
import { Fieldset } from "../forms/Fieldset";
import { FormInput } from "../forms/FormInput";
import { AdminConfigPublic, AdminConfigUpdate } from "../../api-types";

export interface ConnectionTabProps {
  admin: AdminConfigPublic;
  onPatch: (patch: AdminConfigUpdate) => void;
}

type Tri = "default" | "on" | "off";

function triFromOptBool(v: boolean | null | undefined, inverted = false): Tri {
  if (v === null || v === undefined) return "default";
  const enabled = inverted ? !v : v;
  return enabled ? "on" : "off";
}

function applyTri(
  tri: Tri,
  field: keyof AdminConfigUpdate,
  clearField: keyof AdminConfigUpdate,
  inverted = false,
): AdminConfigUpdate {
  if (tri === "default") {
    return { [clearField]: true } as AdminConfigUpdate;
  }
  const enabled = tri === "on";
  const stored = inverted ? !enabled : enabled;
  return { [field]: stored } as AdminConfigUpdate;
}

const TriSelect: React.FC<{
  label: string;
  help: string;
  value: Tri;
  onChange: (v: Tri) => void;
}> = ({ label, help, value, onChange }) => (
  <div className="mb-3">
    <label className="block text-sm text-text mb-1">{label}</label>
    <select
      className="w-full bg-surface border border-divider rounded px-2 py-1.5 text-sm"
      value={value}
      onChange={(e) => onChange(e.target.value as Tri)}
    >
      <option value="default">Use startup default (CLI/env)</option>
      <option value="on">Enabled</option>
      <option value="off">Disabled</option>
    </select>
    <p className="text-sm text-tertiary mt-1">{help}</p>
  </div>
);

export const ConnectionTab: React.FC<ConnectionTabProps> = ({
  admin,
  onPatch,
}) => {
  return (
    <div className="text-secondary py-2 space-y-4">
      <p className="text-sm text-tertiary">
        These write to{" "}
        <code className="bg-surface-sunken px-1 rounded">admin.json</code> and
        apply on the <strong className="text-text">next process restart</strong>.
        Environment variables in the systemd unit override the file.
      </p>

      <Fieldset label="Listening">
        <FormInput
          name="listen_port"
          label="Peer listen port"
          inputType="number"
          value={admin.listen_port?.toString() ?? ""}
          placeholder="e.g. 4241 (empty = startup default)"
          help="TCP/uTP listen port. Clear the field and save to remove the override."
          onChange={(e) => {
            const v = e.target.valueAsNumber;
            if (!e.target.value) {
              onPatch({ clear_listen_port: true });
            } else if (!isNaN(v) && v > 0) {
              onPatch({ listen_port: v });
            }
          }}
        />
        <FormInput
          name="announce_port"
          label="Announce port override"
          inputType="number"
          value={admin.announce_port?.toString() ?? ""}
          placeholder="empty = use listen port"
          help="Port advertised to trackers/DHT when behind NAT/forwarding."
          onChange={(e) => {
            const v = e.target.valueAsNumber;
            if (!e.target.value) {
              onPatch({ clear_announce_port: true });
            } else if (!isNaN(v) && v > 0) {
              onPatch({ announce_port: v });
            }
          }}
        />
        <TriSelect
          label="TCP listen"
          help="Accept incoming peer connections over TCP."
          value={triFromOptBool(admin.disable_tcp_listen, true)}
          onChange={(t) =>
            onPatch(
              applyTri(t, "disable_tcp_listen", "clear_disable_tcp_listen", true),
            )
          }
        />
        <TriSelect
          label="uTP listen (experimental)"
          help="Accept incoming peers over uTP/UDP."
          value={triFromOptBool(admin.enable_utp_listen, false)}
          onChange={(t) =>
            onPatch(
              applyTri(t, "enable_utp_listen", "clear_enable_utp_listen", false),
            )
          }
        />
        <TriSelect
          label="TCP outgoing connects"
          help="Dial peers over TCP (disable if using SOCKS/uTP only)."
          value={triFromOptBool(admin.disable_tcp_connect, true)}
          onChange={(t) =>
            onPatch(
              applyTri(
                t,
                "disable_tcp_connect",
                "clear_disable_tcp_connect",
                true,
              ),
            )
          }
        />
        <TriSelect
          label="UPnP port forwarding"
          help="Ask the gateway to forward the listen port."
          value={triFromOptBool(admin.disable_upnp_port_forward, true)}
          onChange={(t) =>
            onPatch(
              applyTri(
                t,
                "disable_upnp_port_forward",
                "clear_disable_upnp_port_forward",
                true,
              ),
            )
          }
        />
      </Fieldset>

      <Fieldset label="Discovery">
        <TriSelect
          label="DHT"
          help="Distributed Hash Table for peer discovery without trackers."
          value={triFromOptBool(admin.disable_dht, true)}
          onChange={(t) =>
            onPatch(applyTri(t, "disable_dht", "clear_disable_dht", true))
          }
        />
        <TriSelect
          label="DHT persistence"
          help="Remember DHT routing table across restarts."
          value={triFromOptBool(admin.disable_dht_persistence, true)}
          onChange={(t) =>
            onPatch(
              applyTri(
                t,
                "disable_dht_persistence",
                "clear_disable_dht_persistence",
                true,
              ),
            )
          }
        />
        <TriSelect
          label="Local service discovery (LSD)"
          help="Find peers on the local network via multicast."
          value={triFromOptBool(admin.disable_lsd, true)}
          onChange={(t) =>
            onPatch(applyTri(t, "disable_lsd", "clear_disable_lsd", true))
          }
        />
        <TriSelect
          label="Trackers"
          help="Announce to torrent trackers. Private torrents still need trackers."
          value={triFromOptBool(admin.disable_trackers, true)}
          onChange={(t) =>
            onPatch(
              applyTri(t, "disable_trackers", "clear_disable_trackers", true),
            )
          }
        />
      </Fieldset>

      <Fieldset label="Network / proxy">
        <TriSelect
          label="IPv4 only"
          help="Bind and connect using IPv4 only."
          value={triFromOptBool(admin.ipv4_only, false)}
          onChange={(t) =>
            onPatch(applyTri(t, "ipv4_only", "clear_ipv4_only", false))
          }
        />
        <FormInput
          name="bind_device"
          label="Bind device / interface"
          value={admin.bind_device ?? ""}
          placeholder="e.g. eth0 (empty = default)"
          help="SO_BINDTODEVICE / IP_BOUND_IF for torrent traffic."
          onChange={(e) => onPatch({ bind_device: e.target.value })}
        />
        <FormInput
          name="socks_proxy_url"
          label="SOCKS5 proxy URL"
          value={admin.socks_proxy_url ?? ""}
          placeholder="socks5://user:pass@host:port"
          help="Routes outgoing peer connections through the proxy."
          onChange={(e) => onPatch({ socks_proxy_url: e.target.value })}
        />
      </Fieldset>
    </div>
  );
};
