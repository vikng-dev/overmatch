import { exportJWK, generateKeyPair, SignJWT } from "jose";
import { beforeEach, describe, expect, it } from "vitest";
import productionServer from "../production-server.json";
import { accessDenied, adminPage } from "../src/admin";
import { bringUp, call, env, join } from "./helpers";

const TEAM = "https://team.access.test"; // vitest.config.ts ACCESS_TEAM_DOMAIN
// One key pair for the file: jose caches the JWKS per isolate, as production does between rotations.
const keys = generateKeyPair("RS256");

async function token(claims: { aud?: string; iss?: string; expSeconds?: number } = {}): Promise<string> {
	return new SignJWT({ email: "yan@example.test" })
		.setProtectedHeader({ alg: "RS256", kid: "k1" })
		.setIssuer(claims.iss ?? TEAM)
		.setAudience(claims.aud ?? "test-aud")
		.setIssuedAt()
		.setExpirationTime(Math.floor(Date.now() / 1000) + (claims.expSeconds ?? 300))
		.sign((await keys).privateKey);
}

beforeEach(async () => {
	const { publicKey } = await keys;
	edgegap.jwks = { keys: [{ ...(await exportJWK(publicKey)), kid: "k1", alg: "RS256", use: "sig" }] };
});

describe("GET /admin", () => {
	it("refuses a request without an Access token", async () => {
		const res = await call("GET", "/admin");
		expect(res.status).toBe(403);
		expect(await res.text()).toMatch(/missing Cf-Access-Jwt-Assertion/);
	});

	it("refuses a token for another application or issuer, or an expired one", async () => {
		for (const t of [await token({ aud: "other" }), await token({ iss: "https://evil.test" }), await token({ expSeconds: -60 })]) {
			const res = await call("GET", "/admin", undefined, { "cf-access-jwt-assertion": t });
			expect(res.status).toBe(403);
			await res.text();
		}
	});

	it("refuses a correctly signed token that carries no exp", async () => {
		const noExp = await new SignJWT({ email: "yan@example.test" })
			.setProtectedHeader({ alg: "RS256", kid: "k1" })
			.setIssuer(TEAM)
			.setAudience("test-aud")
			.setIssuedAt()
			.sign((await keys).privateKey);
		const res = await call("GET", "/admin", undefined, { "cf-access-jwt-assertion": noExp });
		expect(res.status).toBe(403);
		expect(await res.text()).toMatch(/exp/);
	});

	it("is closed when Access is not configured", async () => {
		const req = new Request("https://x.test/admin", { headers: { "cf-access-jwt-assertion": await token() } });
		expect(await accessDenied(req, { ...env, ACCESS_AUD: "" })).toMatch(/not configured/);
		expect(await accessDenied(req, { ...env, ACCESS_TEAM_DOMAIN: "" })).toMatch(/not configured/);
	});

	it("shows live servers, players, history and derived cost", async () => {
		await join({ version: "0.5.0", locations: ["Paris"], pings: {} });
		await bringUp(0, 3);
		await join({ version: "0.4.0", locations: ["Singapore"], pings: {} });

		const res = await call("GET", "/admin", undefined, { "cf-access-jwt-assertion": await token() });
		expect(res.status).toBe(200);
		expect(res.headers.get("content-type")).toMatch(/text\/html/);
		const html = await res.text();
		expect(html).toContain("<title>Overmatch servers</title>");
		expect(html).toMatch(/<b>2<\/b><span>live servers/);
		expect(html).toMatch(/<b>3<\/b><span>players/);
		expect(html).toContain("198.51.100.1:31001");
		expect(html).toContain("Singapore");
		expect(html).toMatch(/est\. compute this month/);
		expect(html).toContain(`× its recorded vCPU × the current $${env.USD_PER_VCPU_MINUTE}/vCPU-min rate`);
		expect((await env.DB.prepare("SELECT vcpu FROM sessions").all<{ vcpu: number }>()).results.map((r) => r.vcpu))
			.toEqual([productionServer.vcpu, productionServer.vcpu]);
	});

	it("prices each recorded allocation for the admin, including live elapsed time, and counts unpriced sessions by month", async () => {
		// Fixture times, durations and allocations distinguish monthly/all-time and known/unknown totals.
		const now = Date.UTC(2026, 10, 4, 12);
		const monthStart = Date.UTC(2026, 10, 1);
		const fixtures = [
			{ id: "previous-known", at: monthStart - 120 * 60000, minutes: 60, vcpu: 1, live: false },
			{ id: "current-known", at: now - 120 * 60000, minutes: 60, vcpu: 2, live: false },
			{ id: "current-live", at: now - 10 * 60000, minutes: 10, vcpu: 1.5, live: true },
			{ id: "previous-unknown", at: monthStart - 120 * 60000, minutes: 60, vcpu: null, live: false },
			{ id: "current-unknown", at: now - 120 * 60000, minutes: 60, vcpu: null, live: false },
		];
		await env.DB.batch(fixtures.map((r) => env.DB.prepare(
			"INSERT INTO sessions (server_id, version, location, requested_at, ended_at, vcpu) VALUES (?, '0.5.0', ?, ?, ?, ?)",
		).bind(r.id, r.id, r.at, r.live ? null : r.at + r.minutes * 60000, r.vcpu)));

		const html = await adminPage(env, now);
		const rate = Number(env.USD_PER_VCPU_MINUTE);
		expect(html).toContain(`<b>$${((60 * 2 + 10 * 1.5) * rate).toFixed(2)}</b><span>est. compute this month (1 session excluded: vCPU unknown)`);
		expect(html).toContain(`<b>$${((60 * 1 + 60 * 2 + 10 * 1.5) * rate).toFixed(2)}</b><span>est. compute all time (2 sessions excluded: vCPU unknown)`);
		for (const r of fixtures) {
			const row = html.split("<tr>").find((row) => row.includes(`<td>${r.id}</td>`));
			if (r.vcpu === null) {
				expect(row).toContain('unknown (unrecorded)</td><td class="n">unknown</td>');
			} else {
				expect(row).toContain(`<td class="n">${r.vcpu}</td><td class="n">$${(r.minutes * r.vcpu * rate).toFixed(2)}</td>`);
			}
		}
	});

	it("shows the admin an explicitly incomplete zero estimate when every allocation is unrecorded", async () => {
		// Fixture session omits vcpu, as rows written before allocation recording do.
		const now = Date.UTC(2026, 10, 4, 12);
		await env.DB.prepare("INSERT INTO sessions (server_id, version, location, requested_at, ended_at) VALUES ('unrecorded', '0.5.0', 'Paris', ?, ?)")
			.bind(now - 60000, now).run();
		const html = await adminPage(env, now);
		expect(html).toContain('<b>$0.00</b><span>est. compute all time (1 session excluded: vCPU unknown)');
		expect(html).toContain("Sessions with unrecorded vCPU show unknown and are excluded from cost totals.");
	});
});
