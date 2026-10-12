import { runInDurableObject } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import type { LocationsResponse } from "../src/types";
import { bringUp, call, join, registry } from "./helpers";

const V = "0.5.0";

async function locations(version = V): Promise<LocationsResponse> {
	const res = await call("GET", `/v1/locations?version=${version}`);
	if (res.status !== 200) throw new Error(`locations ${res.status}: ${await res.text()}`);
	return (await res.json()) as LocationsResponse;
}

describe("GET /v1/locations", () => {
	it("follows every page and groups by continent", async () => {
		const out = await locations();
		// 15 fixture locations at 4 per page: four pages.
		expect(edgegap.count("GET", "/v1/locations?")).toBe(4);
		expect(out.continents.map((c) => c.continent)).toEqual(["Asia", "Europe", "North America", "Oceania", "South America"]);
		expect(out.continents.flatMap((c) => c.locations).length).toBe(15);
		expect(out.continents[1].locations.map((l) => l.id)).toEqual(["Frankfurt", "London", "Milan", "Paris"]);
	});

	it("attaches a beacon only in the same city and country", async () => {
		const all = (await locations()).continents.flatMap((c) => c.locations);
		const by = (id: string) => all.find((l) => l.id === id)!;
		expect(by("Singapore").beacon).toEqual({ host: "203.0.113.10", fqdn: "sg.beacon.test", udp_port: 5000, tcp_port: 5001 });
		expect(by("Paris").beacon?.host).toBe("203.0.113.13"); // not the Paris, United States beacon
		expect(by("Mumbai").beacon).toBeNull(); // the Jakarta and Singapore beacons are not substitutes
		expect(by("Tokyo").beacon).toBeNull();
	});

	it("counts this version's live matches per location and per continent", async () => {
		await join({ version: V, locations: ["Paris"], pings: {} });
		await join({ version: V, locations: ["Singapore"], pings: {} });
		await bringUp(0, 10);
		await join({ version: V, locations: ["Paris"], pings: {} }); // Paris is full: a second server
		await join({ version: "0.4.0", locations: ["London"], pings: {} });

		const out = await locations();
		const europe = out.continents.find((c) => c.continent === "Europe")!;
		expect(europe.locations.find((l) => l.id === "Paris")!.matches).toBe(2);
		expect(europe.locations.find((l) => l.id === "London")!.matches).toBe(0);
		expect(europe.matches).toBe(2);
		expect(out.continents.find((c) => c.continent === "Asia")!.matches).toBe(1);
		expect((await locations("0.4.0")).continents.find((c) => c.continent === "Europe")!.matches).toBe(1);
	});

	it("caches the provider list and serves it stale when the provider is down", async () => {
		await locations();
		await locations();
		expect(edgegap.count("GET", "/v1/locations/beacons")).toBe(1);

		await runInDurableObject(registry(V), (_, state) => {
			state.storage.sql.exec("UPDATE catalog SET fetched_at = fetched_at - 601000");
		});
		edgegap.listingDown = true;
		expect((await locations()).continents.length).toBe(5);
	});

	it("is 503 when the provider is down and nothing is cached", async () => {
		edgegap.listingDown = true;
		const res = await call("GET", `/v1/locations?version=${V}`);
		expect(res.status).toBe(503);
		await res.text();
	});

	it("requires a valid version", async () => {
		const res = await call("GET", "/v1/locations");
		expect(res.status).toBe(400);
		await res.text();
		const bad = await call("GET", "/v1/locations?version=../x");
		expect(bad.status).toBe(400);
		await bad.text();
	});
});
