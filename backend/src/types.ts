// The /v1 wire contract. `backend/contract/*.json` holds one checked-in example per shape below, and
// test/contract.spec.ts holds those files to these types and to the live handlers. The API is
// additive: a field may be added, never renamed or removed, while any client of that version lives.

/** A deployable placement unit. `id` is what join requests name and what the Edgegap city filter
 *  receives. */
export interface LocationOut {
	id: string;
	city: string;
	country: string;
	continent: string;
	latitude: number;
	longitude: number;
	/** A ping beacon in the same city and country, or null: the client then shows "no estimate".
	 *  Never a nearest-beacon substitute. */
	beacon: BeaconOut | null;
	/** Live servers of the asking version in this location (starting or ready). */
	matches: number;
	// Reserved, absent in v1: `servers?: { ping: { host: string; udp_port: number } }[]`, a
	// per-server ping address for when servers answer pings themselves.
}

export interface BeaconOut {
	host: string;
	fqdn: string | null;
	udp_port: number | null;
	tcp_port: number | null;
}

export interface ContinentOut {
	continent: string;
	/** Sum of `matches` over the locations. Ping range is the client's aggregate: only it measures. */
	matches: number;
	locations: LocationOut[];
}

export interface LocationsResponse {
	version: string;
	continents: ContinentOut[];
}

export interface JoinRequest {
	version: string;
	/** Location ids (`LocationOut.id`) the player accepts. */
	locations: string[];
	/** Client-measured RTT in ms per location id. A location without an estimate is omitted and
	 *  ranks after every location that has one. */
	pings: Record<string, number>;
}

export interface JoinResponse {
	ticket: string;
}

export type TicketState = "starting" | "ready" | "failed";

export interface TicketResponse {
	state: TicketState;
	/** Present only when `state` is "ready". */
	address?: string;
	port?: number;
	/** The match's netcode private key, 64 hex chars (32 bytes). Only when "ready". */
	key?: string;
	/** Present only when `state` is "failed". */
	reason?: string;
}

/** What the server reports. `state` "open" accepts joins; "closing" refuses them (round over, exit
 *  pending). Fullness is derived from `players` and `capacity`, never reported. */
export interface HeartbeatRequest {
	players: number;
	capacity: number;
	state: "open" | "closing";
}

export interface EndRequest {
	reason?: string;
}

export interface ErrorResponse {
	error: string;
}
