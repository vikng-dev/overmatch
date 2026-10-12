// An in-memory Edgegap behind a stubbed global `fetch` (the Registry Durable Object runs in the test
// isolate, so the stub reaches it). It answers only the endpoints src/edgegap.ts calls, plus the
// Access certs URL; any other outbound request fails the test. Location and beacon rows are FIXTURE
// data shaped like the measured 2026-10-12 account listing, not a statement about the live account.

export interface Loc {
	city: string;
	country: string;
	continent: string;
	administrative_division: string;
	timezone: string;
	latitude: number;
	longitude: number;
}

function loc(city: string, country: string, continent: string, latitude: number, longitude: number): Loc {
	return { city, country, continent, administrative_division: "", timezone: "", latitude, longitude };
}

export const FIXTURE_LOCATIONS: Loc[] = [
	loc("Mumbai", "India", "Asia", 19.07, 72.88),
	loc("Singapore", "Singapore", "Asia", 1.35, 103.82),
	loc("Tel Aviv", "Israel", "Asia", 32.08, 34.78),
	loc("Tokyo", "Japan", "Asia", 35.68, 139.69),
	loc("Frankfurt", "Germany", "Europe", 50.11, 8.68),
	loc("London", "United Kingdom", "Europe", 51.51, -0.13),
	loc("Milan", "Italy", "Europe", 45.46, 9.19),
	loc("Paris", "France", "Europe", 48.86, 2.35),
	loc("Ashburn", "United States", "North America", 39.04, -77.49),
	loc("Chicago", "United States", "North America", 41.88, -87.63),
	loc("Los Angeles", "United States", "North America", 34.05, -118.24),
	loc("Miami", "United States", "North America", 25.76, -80.19),
	loc("Seattle", "United States", "North America", 47.61, -122.33),
	loc("Melbourne", "Australia", "Oceania", -37.81, 144.96),
	loc("Sao Paulo", "Brazil", "South America", -23.55, -46.63),
];

export const FIXTURE_BEACONS = [
	{ host: "203.0.113.10", fqdn: "sg.beacon.test", udp_port: 5000, tcp_port: 5001, location: loc("Singapore", "Singapore", "Asia", 1.3, 103.8) },
	{ host: "203.0.113.11", fqdn: "jk.beacon.test", udp_port: 5000, tcp_port: 5001, location: loc("Jakarta", "Indonesia", "Asia", -6.2, 106.8) },
	{ host: "203.0.113.12", fqdn: "fr.beacon.test", udp_port: 5000, tcp_port: 5001, location: loc("Frankfurt", "Germany", "Europe", 50.1, 8.7) },
	// Same city name, different country, listed first: must NOT attach to Paris, France.
	{ host: "203.0.113.14", fqdn: "tx.beacon.test", udp_port: 5000, tcp_port: 5001, location: loc("Paris", "United States", "North America", 33.7, -95.6) },
	{ host: "203.0.113.13", fqdn: "pa.beacon.test", udp_port: 5000, tcp_port: 5001, location: loc("Paris", "France", "Europe", 48.9, 2.4) },
];

export interface Deployment {
	id: string;
	body: Record<string, unknown>;
	status: "Status.DEPLOYING" | "Status.READY" | "Status.ERROR" | "Status.TERMINATED";
	publicIp: string;
	port: number;
}

export class FakeEdgegap {
	readonly base = "https://edgegap.test";
	locations: Loc[] = FIXTURE_LOCATIONS;
	beacons = FIXTURE_BEACONS;
	readonly deployments = new Map<string, Deployment>();
	readonly calls: { method: string; path: string }[] = [];
	/** Cities whose deploy is refused synchronously (HTTP 422), as a full location is. */
	refuse = new Set<string>();
	/** Error bodies and error_detail echo the request back, env values and webhook URL included:
	 *  the worst case for what a provider error may carry. */
	echoInErrors = false;
	/** Server-side page-size cap, so a client asking for more must still follow `has_next`. */
	maxPageSize = 4;
	/** When set, the location and beacon endpoints answer 500. */
	listingDown = false;
	/** Access JWKS served at the certs URL. */
	jwks: { keys: unknown[] } = { keys: [] };
	private gate: Promise<void> | null = null;
	private seq = 0;

	/** The next this-many deploy calls create the deployment, then answer 502: the response is lost. */
	loseDeployResponses = 0;
	/** The next this-many deploy calls answer 502 and create nothing. */
	failDeploysUnanswered = 0;
	/** Deployment ids whose status call never answers. */
	hangStatus = new Set<string>();
	/** The next this-many stop calls answer 503 and stop nothing. */
	failStops = 0;
	/** Deployment ids whose stop call never answers. */
	hangStops = new Set<string>();
	private stopGate: Promise<void> | null = null;

	/** Hold every stop response until the returned function is called. */
	holdStops(): () => void {
		let release!: () => void;
		this.stopGate = new Promise((r) => (release = r));
		return () => {
			this.stopGate = null;
			release();
		};
	}

	/** Hold every deploy response until the returned function is called. */
	holdDeploys(): () => void {
		let release!: () => void;
		this.gate = new Promise((r) => (release = r));
		return () => {
			this.gate = null;
			release();
		};
	}

	count(method: string, prefix: string): number {
		return this.calls.filter((c) => c.method === method && c.path.startsWith(prefix)).length;
	}

	deployment(n = 0): Deployment {
		const d = [...this.deployments.values()][n];
		if (!d) throw new Error(`no deployment #${n}`);
		return d;
	}

	env(d: Deployment, key: string): string {
		const vars = d.body.env_vars as { key: string; value: string }[];
		const v = vars.find((e) => e.key === key);
		if (!v) throw new Error(`deployment ${d.id} has no ${key}`);
		return v.value;
	}

	/** A running deployment this Worker did not start in this test, carrying `tags`. */
	foreign(tags: string[]): Deployment {
		const id = `dep${++this.seq}`;
		const d: Deployment = { id, body: { tags, env_vars: [] }, status: "Status.READY", publicIp: `198.51.100.${this.seq}`, port: 31000 + this.seq };
		this.deployments.set(id, d);
		return d;
	}

	ready(d: Deployment): void {
		d.status = "Status.READY";
	}

	readonly fetch = async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
		// A macrotask, as real network I/O is: other requests to the registry may run meanwhile.
		await new Promise((r) => setTimeout(r, 1));
		const req = new Request(input, init);
		const url = new URL(req.url);
		if (url.origin === "https://team.access.test" && url.pathname === "/cdn-cgi/access/certs") {
			return Response.json(this.jwks);
		}
		if (url.origin !== this.base) throw new Error(`unexpected outbound fetch: ${req.method} ${req.url}`);
		if (req.headers.get("authorization") !== "token test-token") return Response.json({ message: "unauthorized" }, { status: 401 });
		this.calls.push({ method: req.method, path: url.pathname + url.search });

		if (req.method === "GET" && url.pathname === "/v1/locations") {
			const page = Number(url.searchParams.get("page") ?? "1");
			if (this.listingDown) return Response.json({ message: "down" }, { status: 500 });
			const limit = Math.min(this.maxPageSize, Number(url.searchParams.get("limit") ?? "10"));
			const slice = this.locations.slice((page - 1) * limit, page * limit);
			const pages = Math.max(1, Math.ceil(this.locations.length / limit));
			return Response.json({
				locations: slice,
				total_count: this.locations.length,
				pagination: {
					number: page,
					next_page_number: page < pages ? page + 1 : null,
					previous_page_number: page > 1 ? page - 1 : null,
					paginator: { num_pages: pages },
					has_next: page < pages,
					has_previous: page > 1,
				},
			});
		}
		if (req.method === "GET" && url.pathname === "/v1/locations/beacons") {
			if (this.listingDown) return Response.json({ message: "down" }, { status: 500 });
			return Response.json({ locations: this.beacons, count: this.beacons.length });
		}
		if (req.method === "POST" && url.pathname === "/v1/deploy") {
			const body = (await req.json()) as Record<string, unknown>;
			if (this.gate) await this.gate;
			const city = (body.filters as { values: string[] }[])[0].values[0];
			if (this.refuse.has(city)) {
				const echo = this.echoInErrors ? ` request: ${JSON.stringify(body)}` : "";
				return Response.json({ message: `Not enough space on locations or no locations available${echo}` }, { status: 422 });
			}
			if (this.failDeploysUnanswered > 0) {
				this.failDeploysUnanswered--;
				return Response.json({ message: "bad gateway" }, { status: 502 });
			}
			const id = `dep${++this.seq}`;
			this.deployments.set(id, { id, body, status: "Status.DEPLOYING", publicIp: `198.51.100.${this.seq}`, port: 31000 + this.seq });
			if (this.loseDeployResponses > 0) {
				this.loseDeployResponses--;
				return Response.json({ message: "bad gateway" }, { status: 502 });
			}
			return Response.json({ request_id: id, request_dns: `${id}.pr.edgegap.test`, request_app: body.app_name, request_version: body.version_name, request_user_count: 1, filters: body.filters });
		}
		if (req.method === "GET" && url.pathname === "/v1/deployments") {
			// Only the filter src/edgegap.ts sends: one `tags eq` filter. Lists active deployments, paged
			// like /v1/locations.
			const query = JSON.parse(url.searchParams.get("query") ?? "{}") as { filters?: { field: string; operator: string; value: string }[] };
			const tag = query.filters?.find((f) => f.field === "tags" && f.operator === "eq")?.value;
			const all = [...this.deployments.values()]
				.filter((d) => d.status === "Status.DEPLOYING" || d.status === "Status.READY")
				.filter((d) => tag === undefined || ((d.body.tags as string[] | undefined) ?? []).includes(tag))
				.map((d) => ({ request_id: d.id, fqdn: `${d.id}.pr.edgegap.test`, ready: d.status === "Status.READY", start_time: "", public_ip: d.publicIp, tags: d.body.tags ?? [] }));
			const page = Number(url.searchParams.get("page") ?? "1");
			const limit = Math.min(this.maxPageSize, Number(url.searchParams.get("limit") ?? "10"));
			const pages = Math.max(1, Math.ceil(all.length / limit));
			return Response.json({
				data: all.slice((page - 1) * limit, page * limit),
				total_count: all.length,
				pagination: { number: page, next_page_number: page < pages ? page + 1 : null, previous_page_number: page > 1 ? page - 1 : null, paginator: { num_pages: pages }, has_next: page < pages, has_previous: page > 1 },
			});
		}
		let m = /^\/v1\/status\/(.+)$/.exec(url.pathname);
		if (req.method === "GET" && m) {
			if (this.hangStatus.has(m[1])) return new Promise<Response>(() => {});
			const d = this.deployments.get(m[1]);
			if (!d) return Response.json({ message: "not found" }, { status: 404 });
			const ready = d.status === "Status.READY";
			return Response.json({
				request_id: d.id,
				fqdn: `${d.id}.pr.edgegap.test`,
				public_ip: d.publicIp,
				app_name: d.body.app_name,
				app_version: d.body.version_name,
				current_status: d.status,
				running: ready,
				start_time: "2026-10-12T00:00:00Z",
				elapsed_time: 0,
				max_duration: 30,
				error: d.status === "Status.ERROR",
				error_detail: d.status === "Status.ERROR" ? `Not enough space on locations${this.echoInErrors ? ` ${JSON.stringify(d.body)}` : ""}` : "",
				ports: { game: { name: "game", internal: 5888, external: d.port, protocol: "UDP", link: `${d.id}:${d.port}`, tls_upgrade: false } },
				tags: d.body.tags ?? [],
			});
		}
		m = /^\/v1\/stop\/(.+)$/.exec(url.pathname);
		if (req.method === "DELETE" && m) {
			if (this.hangStops.has(m[1])) return new Promise<Response>(() => {});
			if (this.stopGate) await this.stopGate;
			if (this.failStops > 0) {
				this.failStops--;
				return Response.json({ message: "unavailable" }, { status: 503 });
			}
			const d = this.deployments.get(m[1]);
			if (!d) return Response.json({ message: "not found" }, { status: 404 });
			d.status = "Status.TERMINATED";
			return Response.json({ message: "stopping" }, { status: 202 });
		}
		throw new Error(`fake edgegap: unhandled ${req.method} ${url.pathname}`);
	};
}
