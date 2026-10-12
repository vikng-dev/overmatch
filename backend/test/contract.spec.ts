// Holds contract/*.json to the live handlers: every request example is accepted, and every live
// response has exactly the keys and value types of its example (recursively; an array element must
// match some example element; null matches null). A field added on either side without the other
// fails here.

import { describe, expect, it } from "vitest";
import endReq from "../contract/end.request.json";
import errorRes from "../contract/error.response.json";
import heartbeatReq from "../contract/heartbeat.request.json";
import joinReq from "../contract/join.request.json";
import joinRes from "../contract/join.response.json";
import locationsRes from "../contract/locations.response.json";
import serverEnv from "../contract/server-env.json";
import failedRes from "../contract/ticket.failed.response.json";
import readyRes from "../contract/ticket.ready.response.json";
import startingRes from "../contract/ticket.starting.response.json";
import { age, call, join, launched, registry, webhook } from "./helpers";
import { runDurableObjectAlarm } from "cloudflare:test";

function kind(v: unknown): string {
	return v === null ? "null" : Array.isArray(v) ? "array" : typeof v;
}

function mismatch(live: unknown, example: unknown, path = "$"): string | null {
	if (kind(live) !== kind(example)) {
		return `${path}: live ${kind(live)} vs example ${kind(example)}`;
	}
	if (Array.isArray(live)) {
		const ex = example as unknown[];
		for (const [i, el] of live.entries()) {
			const errs = ex.map((e) => mismatch(el, e, `${path}[${i}]`));
			if (!errs.some((e) => e === null)) return errs[0] ?? `${path}[${i}]: example array is empty`;
		}
		return null;
	}
	if (kind(live) === "object") {
		const a = Object.keys(live as object).sort();
		const b = Object.keys(example as object).sort();
		if (a.join() !== b.join()) return `${path}: live keys [${a}] vs example keys [${b}]`;
		for (const k of a) {
			const e = mismatch((live as Record<string, unknown>)[k], (example as Record<string, unknown>)[k], `${path}.${k}`);
			if (e) return e;
		}
	}
	return null;
}

function conforms(live: unknown, example: unknown): void {
	expect(mismatch(live, example)).toBeNull();
}

describe("contract/*.json matches the handlers", () => {
	it("locations", async () => {
		await join(joinReq);
		const res = await call("GET", "/v1/locations?version=0.5.0");
		conforms(await res.json(), locationsRes);
	});

	it("join request, join response, starting and ready tickets", async () => {
		const res = await call("POST", "/v1/join", joinReq);
		expect(res.status).toBe(200);
		const body = (await res.json()) as { ticket: string };
		conforms(body, joinRes);
		conforms(await (await call("GET", `/v1/join/${body.ticket}`)).json(), startingRes);

		const s = launched();
		edgegap.ready(edgegap.deployment(0));
		await webhook(s.id);
		const hb = await call("POST", `/v1/servers/${s.id}/heartbeat`, heartbeatReq, { authorization: `Bearer ${s.secret}` });
		expect(hb.status).toBe(204);
		conforms(await (await call("GET", `/v1/join/${body.ticket}`)).json(), readyRes);

		const end = await call("POST", `/v1/servers/${s.id}/end`, endReq, { authorization: `Bearer ${s.secret}` });
		expect(end.status).toBe(204);
	});

	it("failed ticket", async () => {
		const t = await join({ version: "0.5.0", locations: ["Paris"], pings: {} });
		await age("0.5.0", 61_000);
		await runDurableObjectAlarm(registry("0.5.0"));
		conforms(await (await call("GET", `/v1/join/${t}`)).json(), failedRes);
	});

	it("error", async () => {
		const res = await call("POST", "/v1/join", { version: "0.5.0", locations: ["Atlantis"], pings: {} });
		expect(res.status).toBe(400);
		const body = await res.json();
		conforms(body, errorRes);
		expect(body).toEqual(errorRes);
	});

	it("server env: exactly the OVERMATCH_* variables a deploy passes", async () => {
		await join(joinReq);
		const vars = edgegap.deployment(0).body.env_vars as { key: string; value: string }[];
		expect(vars.map((v) => v.key).sort()).toEqual(Object.keys(serverEnv).sort());
	});
});
