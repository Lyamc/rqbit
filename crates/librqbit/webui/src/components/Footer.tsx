import { useContext, useEffect, useState } from "react";
import { APIContext } from "../context";
import { PublicIpFamily, PublicIpInfo } from "../api-types";
import { formatBytes } from "../helper/formatBytes";
import { formatSecondsToTime } from "../helper/formatSecondsToTime";
import { useStatsStore } from "../stores/statsStore";

const FooterPiece: React.FC<{ children: React.ReactNode; title?: string }> = ({
  children,
  title,
}) => {
  return (
    <div className="p-1" title={title}>
      {children}
    </div>
  );
};

const famLine = (name: string, f: PublicIpFamily) =>
  f.ip
    ? `${name}: ${f.ip}${f.source ? ` (via ${f.source})` : ""}`
    : f.error
      ? `${name}: none (${f.error})`
      : `${name}: not checked`;

/** Server's public address (its own egress), polled every 60 s. */
const usePublicIp = (): PublicIpInfo | null => {
  const API = useContext(APIContext);
  const [info, setInfo] = useState<PublicIpInfo | null>(null);
  useEffect(() => {
    if (!API.getPublicIp) return;
    let stopped = false;
    const load = () =>
      API.getPublicIp!()
        .then((i) => !stopped && setInfo(i))
        .catch(() => {});
    load();
    const t = setInterval(load, 60000);
    return () => {
      stopped = true;
      clearInterval(t);
    };
  }, [API]);
  return info;
};

export const Footer: React.FC<{}> = () => {
  const API = useContext(APIContext);
  let stats = useStatsStore((stats) => stats.stats);
  const publicIp = usePublicIp();
  const target = API.getConnectionTarget?.() ?? null;

  const ips = publicIp?.enabled
    ? [publicIp.ipv4.ip, publicIp.ipv6.ip].filter((x): x is string => !!x)
    : [];
  const ipTitle = publicIp?.enabled
    ? [
        "Server's public address (its own egress, e.g. the VPN exit)",
        famLine("IPv4", publicIp.ipv4),
        famLine("IPv6", publicIp.ipv6),
        publicIp.checked_at
          ? `checked ${new Date(publicIp.checked_at).toLocaleString()}`
          : "not checked yet",
      ].join("\n")
    : undefined;

  return (
    <div className="sticky bottom-0 bg-surface-raised/80 backdrop-blur text-nowrap text-sm font-medium text-secondary flex gap-x-1 lg:gap-x-5 justify-evenly flex-wrap">
      {target && (
        <FooterPiece
          title={`${window.location.origin}\nBrowsers don't reveal the resolved server IP or the local port of a connection to web pages, so only the host is shown.`}
        >
          {target}
        </FooterPiece>
      )}
      <FooterPiece>
        ↓ {stats.download_speed.human_readable} (
        {formatBytes(stats.counters.fetched_bytes)})
      </FooterPiece>
      <FooterPiece>
        ↑ {stats.upload_speed.human_readable} (
        {formatBytes(stats.counters.uploaded_bytes)})
      </FooterPiece>
      <FooterPiece>up {formatSecondsToTime(stats.uptime_seconds)}</FooterPiece>
      {publicIp?.enabled && (
        <FooterPiece title={ipTitle}>
          {ips.length > 0
            ? `public ${ips.join(" · ")}`
            : publicIp.checked_at
              ? "public IP: unknown"
              : "public IP: checking…"}
        </FooterPiece>
      )}
    </div>
  );
};
