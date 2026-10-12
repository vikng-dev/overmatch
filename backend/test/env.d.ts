// Types `exports.default` (cloudflare:workers) from the Worker's own module.
declare namespace Cloudflare {
	interface GlobalProps {
		mainModule: typeof import("../src/index");
		durableNamespaces: "Registry";
	}
}
