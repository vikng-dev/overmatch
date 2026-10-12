// Server ids and join tickets carry their game version, so a request that names only an id still
// reaches the right registry: `<base64url(version)>.<32 random hex>`. The random half is 128 bits;
// a ticket is a bearer capability for its server's netcode key.

const VERSION_RE = /^[A-Za-z0-9][A-Za-z0-9._+-]{0,63}$/;

export function validVersion(v: unknown): v is string {
	return typeof v === "string" && VERSION_RE.test(v);
}

export function randomHex(bytes: number): string {
	const b = crypto.getRandomValues(new Uint8Array(bytes));
	return Array.from(b, (x) => x.toString(16).padStart(2, "0")).join("");
}

function b64url(s: string): string {
	return btoa(String.fromCharCode(...new TextEncoder().encode(s)))
		.replaceAll("+", "-")
		.replaceAll("/", "_")
		.replace(/=+$/, "");
}

function unb64url(s: string): string | null {
	try {
		const bin = atob(s.replaceAll("-", "+").replaceAll("_", "/"));
		return new TextDecoder("utf-8", { fatal: true, ignoreBOM: false }).decode(Uint8Array.from(bin, (c) => c.charCodeAt(0)));
	} catch {
		return null;
	}
}

export function newId(version: string): string {
	return idOf(version, randomHex(16));
}

/** The id whose random half is `hex`, minted for `version`. */
export function idOf(version: string, hex: string): string {
	return `${b64url(version)}.${hex}`;
}

/** The version an id was minted for, or null if the id is malformed. */
export function versionOf(id: string): string | null {
	const m = /^([A-Za-z0-9_-]{1,96})\.[0-9a-f]{32}$/.exec(id);
	if (!m) return null;
	const v = unb64url(m[1]);
	return validVersion(v) ? v : null;
}

export async function sha256Hex(s: string): Promise<string> {
	const d = new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(s)));
	return Array.from(d, (x) => x.toString(16).padStart(2, "0")).join("");
}

export function timingSafeEqual(a: string, b: string): boolean {
	if (a.length !== b.length) return false;
	let diff = 0;
	for (let i = 0; i < a.length; i++) diff |= a.charCodeAt(i) ^ b.charCodeAt(i);
	return diff === 0;
}
