import { exportJWK, generateKeyPair, SignJWT } from "jose";
import { beforeEach, describe, expect, it } from "vitest";
import { accessDenied } from "../src/admin";
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
	});
});
