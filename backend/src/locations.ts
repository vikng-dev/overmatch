import type { Beacon, LocationData } from "./edgegap";
import type { BeaconOut, ContinentOut } from "./types";

/** A location's id is its city: the value Edgegap's deploy `filters` takes. */
export function locationId(l: LocationData): string {
	return l.city;
}

function sameCity(a: LocationData, b: LocationData): boolean {
	const norm = (s: string) => s.normalize("NFKC").trim().toLowerCase();
	return norm(a.city) === norm(b.city) && norm(a.country) === norm(b.country);
}

/** Continent groups sorted by name, locations sorted by city. A location carries a beacon only
 *  when one sits in the same city and country; ordering by ping is the client's, since only it
 *  measures. */
export function groupLocations(locations: LocationData[], beacons: Beacon[], matches: Map<string, number>): ContinentOut[] {
	const byId = new Map<string, LocationData>();
	for (const l of locations) byId.set(locationId(l), l);

	const groups = new Map<string, ContinentOut>();
	for (const l of [...byId.values()].sort((a, b) => a.city.localeCompare(b.city))) {
		const b = beacons.find((x) => sameCity(x.location, l));
		const beacon: BeaconOut | null = b ? { host: b.host, fqdn: b.fqdn, udp_port: b.udp_port, tcp_port: b.tcp_port } : null;
		const m = matches.get(locationId(l)) ?? 0;
		let g = groups.get(l.continent);
		if (!g) {
			g = { continent: l.continent, matches: 0, locations: [] };
			groups.set(l.continent, g);
		}
		g.matches += m;
		g.locations.push({
			id: locationId(l),
			city: l.city,
			country: l.country,
			continent: l.continent,
			latitude: l.latitude,
			longitude: l.longitude,
			beacon,
			matches: m,
		});
	}
	return [...groups.values()].sort((a, b) => a.continent.localeCompare(b.continent));
}
