import { ProviderError } from "./provider";

/** Only EC2 Query XML: no DTD, external entities or mixed content. Malformed input is unknown fate. */
export class Xml {
	readonly children: Xml[] = [];
	text = "";
	constructor(readonly name: string) {}
	all(name: string): Xml[] { return this.children.filter((c) => c.name === name); }
	one(name: string): Xml {
		const found = this.all(name);
		if (found.length !== 1) throw new ProviderError("xml", 0, "unknown");
		return found[0];
	}
	optional(name: string): string | null {
		const found = this.all(name);
		if (found.length > 1) throw new ProviderError("xml", 0, "unknown");
		return found.length ? found[0].text : null;
	}
}

export function parseXml(raw: string): Xml {
	const bad = () => { throw new ProviderError("xml", 0, "unknown"); };
	if (/<!/.test(raw)) bad();
	const root = new Xml("document");
	const stack = [root];
	const tokens = raw.match(/<[^>]*>|[^<]+/g) ?? [];
	if (tokens.join("") !== raw) bad();
	for (const token of tokens) {
		if (/^<\?xml\s[^?]*\?>$/.test(token)) continue;
		if (token.startsWith("</")) {
			if (stack.length < 2 || token !== `</${stack.pop()!.name}>`) bad();
		} else if (token.startsWith("<")) {
			const m = /^<([A-Za-z][A-Za-z0-9]*)(?:\s+xmlns="[^"]*")?\s*(\/?)>$/.exec(token);
			if (!m) bad();
			const child = new Xml(m![1]);
			stack[stack.length - 1].children.push(child);
			if (!m![2]) stack.push(child);
		} else {
			if (/&(?!amp;|lt;|gt;|quot;|apos;|#\d+;|#x[0-9a-fA-F]+;)/.test(token)) bad();
			const text = token.replace(/&([^;]+);/g, (_, entity: string) => {
				if (entity.startsWith("#")) {
					const n = entity[1] === "x" ? parseInt(entity.slice(2), 16) : Number(entity.slice(1));
					if (n < 1 || n > 0x10ffff || (n >= 0xd800 && n <= 0xdfff)) bad();
					return String.fromCodePoint(n);
				}
				return ({ amp: "&", lt: "<", gt: ">", quot: '"', apos: "'" } as Record<string, string>)[entity];
			});
			stack[stack.length - 1].text += text;
		}
	}
	if (stack.length !== 1 || root.children.length !== 1 || root.text.trim()) bad();
	return root.children[0];
}
