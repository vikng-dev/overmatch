// GET /admin: live servers (from each live version's Registry snapshot) plus history and cost (from
// D1). Cloudflare Access sits in front of the route; this module also verifies the Access JWT, and
// with ACCESS_TEAM_DOMAIN or ACCESS_AUD unset it refuses every request (fail closed).

import { createRemoteJWKSet, jwtVerify } from "jose";
import { config, type Env } from "./config";
import { registryFor, type ServerSnapshot } from "./registry";

/** Concurrent Registry calls per request; the platform allows six outgoing connections. */
const FANOUT = 5;

let jwks: { url: string; set: ReturnType<typeof createRemoteJWKSet> } | null = null;

export async function accessDenied(req: Request, env: Env): Promise<string | null> {
	const team = env.ACCESS_TEAM_DOMAIN?.replace(/\/+$/, "");
	if (!team || !env.ACCESS_AUD) return "admin disabled: ACCESS_TEAM_DOMAIN and ACCESS_AUD are not configured";
	const token = req.headers.get("cf-access-jwt-assertion");
	if (!token) return "missing Cf-Access-Jwt-Assertion";
	const url = `${team}/cdn-cgi/access/certs`;
	if (jwks?.url !== url) jwks = { url, set: createRemoteJWKSet(new URL(url)) };
	try {
		await jwtVerify(token, jwks.set, { issuer: team, audience: env.ACCESS_AUD });
		return null;
	} catch (err) {
		return `invalid Access token: ${err instanceof Error ? err.message : String(err)}`;
	}
}

interface SessionRow {
	server_id: string;
	version: string;
	location: string;
	requested_at: number;
	ready_at: number | null;
	ended_at: number | null;
	outcome: string | null;
	reason: string | null;
	peak_players: number;
	minutes: number | null;
}

export async function adminPage(env: Env, now = Date.now()): Promise<string> {
	const cfg = config(env);
	const monthStart = Date.UTC(new Date(now).getUTCFullYear(), new Date(now).getUTCMonth(), 1);

	const [liveVersions, totals, recent] = await Promise.all([
		env.DB.prepare("SELECT DISTINCT version FROM sessions WHERE ended_at IS NULL ORDER BY version").all<{ version: string }>(),
		env.DB.prepare(
			`SELECT
				COALESCE(SUM(CASE WHEN ended_at IS NULL THEN (?1 - requested_at) / 60000.0 ELSE minutes END), 0) AS all_minutes,
				COALESCE(SUM(CASE WHEN requested_at >= ?2 THEN
					CASE WHEN ended_at IS NULL THEN (?1 - requested_at) / 60000.0 ELSE minutes END ELSE 0 END), 0) AS month_minutes,
				COUNT(*) AS sessions,
				COALESCE(SUM(CASE WHEN outcome = 'failed' THEN 1 ELSE 0 END), 0) AS failures
			 FROM sessions`,
		)
			.bind(now, monthStart)
			.first<{ all_minutes: number; month_minutes: number; sessions: number; failures: number }>(),
		env.DB.prepare("SELECT * FROM sessions ORDER BY requested_at DESC LIMIT 30").all<SessionRow>(),
	]);

	const versions = liveVersions.results.map((r) => r.version);
	const live: { version: string; servers: ServerSnapshot[] | null }[] = [];
	for (let i = 0; i < versions.length; i += FANOUT) {
		const batch = versions.slice(i, i + FANOUT);
		const snaps = await Promise.all(
			batch.map((v) =>
				registryFor(env, v)
					.snapshot()
					.then((s) => s.filter((x) => x.state !== "ended"))
					.catch(() => null),
			),
		);
		batch.forEach((version, j) => live.push({ version, servers: snaps[j] }));
	}

	const usd = (min: number) => (min * cfg.vcpuPerServer * cfg.usdPerVcpuMinute).toFixed(2);
	const t = totals ?? { all_minutes: 0, month_minutes: 0, sessions: 0, failures: 0 };
	const allServers = live.flatMap((l) => l.servers ?? []);
	const players = allServers.reduce((n, s) => n + (s.state === "ready" ? s.players : 0), 0);

	const liveRows = live
		.flatMap(({ version, servers }) =>
			servers === null
				? [`<tr><td>${esc(version)}</td><td colspan="6">snapshot unavailable</td></tr>`]
				: servers.map(
						(s) =>
							`<tr><td>${esc(version)}</td><td>${esc(s.location)}</td><td>${esc(s.state)}${s.server_state === "closing" ? " (closing)" : ""}</td>` +
							`<td class="n">${s.players}/${s.capacity}</td><td>${s.address ? esc(`${s.address}:${s.port}`) : "-"}</td>` +
							`<td class="n">${ago(now, s.created_at)}</td><td class="n">${s.last_seen ? ago(now, s.last_seen) : "-"}</td></tr>`,
					),
		)
		.join("");

	const recentRows = recent.results
		.map(
			(r) =>
				`<tr><td>${iso(r.requested_at)}</td><td>${esc(r.version)}</td><td>${esc(r.location)}</td>` +
				`<td>${r.ended_at === null ? "live" : esc(r.outcome ?? "")}</td><td class="n">${r.peak_players}</td>` +
				`<td class="n">${r.minutes === null ? "-" : r.minutes.toFixed(1)}</td><td>${esc(r.reason ?? "")}</td></tr>`,
		)
		.join("");

	return `<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Overmatch servers</title>
<style>
:root{color-scheme:light dark;--fg:#1b1d21;--bg:#fafafa;--mute:#6b7079;--line:#d9dce1}
@media (prefers-color-scheme:dark){:root{--fg:#e6e8eb;--bg:#15171a;--mute:#9298a1;--line:#2c3036}}
body{font:14px/1.45 system-ui,sans-serif;color:var(--fg);background:var(--bg);margin:0 auto;padding:16px;max-width:1100px}
h1{font-size:20px}h2{font-size:15px;margin-top:28px}
.kpi{display:flex;gap:24px;flex-wrap:wrap}.kpi div{min-width:120px}.kpi b{display:block;font-size:22px}
.kpi span{color:var(--mute);font-size:12px}
.wrap{overflow-x:auto}table{border-collapse:collapse;width:100%}
th,td{text-align:left;padding:4px 8px;border-bottom:1px solid var(--line);white-space:nowrap}
th{color:var(--mute);font-weight:500}.n{text-align:right;font-variant-numeric:tabular-nums}
</style></head><body>
<h1>Overmatch servers</h1>
<div class="kpi">
<div><b>${allServers.length}</b><span>live servers</span></div>
<div><b>${players}</b><span>players</span></div>
<div><b>${t.month_minutes.toFixed(0)}</b><span>server-minutes this month</span></div>
<div><b>$${usd(t.month_minutes)}</b><span>est. compute this month</span></div>
<div><b>$${usd(t.all_minutes)}</b><span>est. compute all time</span></div>
<div><b>${t.failures}/${t.sessions}</b><span>failed starts / sessions</span></div>
</div>
<h2>Live</h2>
<div class="wrap"><table><tr><th>version</th><th>location</th><th>state</th><th class="n">players</th><th>address</th><th class="n">age</th><th class="n">last heartbeat</th></tr>
${liveRows || '<tr><td colspan="7">none</td></tr>'}</table></div>
<h2>Recent sessions</h2>
<div class="wrap"><table><tr><th>requested (UTC)</th><th>version</th><th>location</th><th>outcome</th><th class="n">peak</th><th class="n">minutes</th><th>reason</th></tr>
${recentRows || '<tr><td colspan="7">none</td></tr>'}</table></div>
<p style="color:var(--mute)">Cost is DERIVED: minutes from request to end × ${cfg.vcpuPerServer} vCPU × $${cfg.usdPerVcpuMinute}/vCPU-min, compute only (egress not included). Reconcile against the Edgegap invoice.</p>
</body></html>`;
}

function esc(s: string): string {
	return s.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!);
}

function iso(ms: number): string {
	return new Date(ms).toISOString().replace("T", " ").slice(0, 19);
}

function ago(now: number, then: number): string {
	const s = Math.max(0, Math.round((now - then) / 1000));
	return s < 120 ? `${s}s` : s < 7200 ? `${Math.round(s / 60)}m` : `${(s / 3600).toFixed(1)}h`;
}
