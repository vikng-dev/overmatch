import { sha256Hex } from "./ids";

const encode = (s: string) => encodeURIComponent(s).replace(/[!'()*]/g, (c) => `%${c.charCodeAt(0).toString(16).toUpperCase()}`);
export function queryString(fields: Record<string, string>): string {
	return Object.entries(fields).map(([k, v]) => [encode(k), encode(v)])
		.sort(([a, av], [b, bv]) => a < b ? -1 : a > b ? 1 : av < bv ? -1 : av > bv ? 1 : 0)
		.map(([k, v]) => `${k}=${v}`).join("&");
}

async function hmac(key: Uint8Array, text: string): Promise<Uint8Array<ArrayBuffer>> {
	const k = await crypto.subtle.importKey("raw", new Uint8Array(key), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
	return new Uint8Array(await crypto.subtle.sign("HMAC", k, new TextEncoder().encode(text)));
}

/** SigV4 canonical request and signing chain, per AWS IAM's signed-request reference.
 * Header names are lowercase; callers supply the exact body that fetch sends. Never log the result. */
export async function signV4(input: {
	method: string; url: URL; body: string; headers: Record<string, string>;
	accessKey: string; secretKey: string; region: string; service: string; date: Date;
}): Promise<Record<string, string>> {
	const timestamp = input.date.toISOString().replace(/[:-]|\.\d{3}/g, "");
	const day = timestamp.slice(0, 8);
	const headers = { ...input.headers, host: input.url.host, "x-amz-date": timestamp };
	const names = Object.keys(headers).sort() as (keyof typeof headers)[];
	const signedHeaders = names.join(";");
	const canonical = [input.method, input.url.pathname, queryString(Object.fromEntries(input.url.searchParams)),
		names.map((name) => `${name}:${headers[name].trim().replace(/\s+/g, " ")}\n`).join(""),
		signedHeaders, await sha256Hex(input.body)].join("\n");
	const scope = `${day}/${input.region}/${input.service}/aws4_request`;
	const stringToSign = `AWS4-HMAC-SHA256\n${timestamp}\n${scope}\n${await sha256Hex(canonical)}`;
	let key = new TextEncoder().encode(`AWS4${input.secretKey}`);
	for (const part of [day, input.region, input.service, "aws4_request"]) key = await hmac(key, part);
	const signature = Array.from(await hmac(key, stringToSign), (b) => b.toString(16).padStart(2, "0")).join("");
	return { ...headers, authorization: `AWS4-HMAC-SHA256 Credential=${input.accessKey}/${scope}, SignedHeaders=${signedHeaders}, Signature=${signature}` };
}
