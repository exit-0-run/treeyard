// Resolve hook for the T2 deciding tests on astro: Node strips TypeScript types, but the sources
// import './x.js' and the package tests import '../dist/x.js'. Both map to the '.ts' source here,
// so the tests need no `pnpm install` and no build. Use: node --import <this file> --test FILE.
import { registerHooks } from 'node:module';
import { existsSync } from 'node:fs';
import { fileURLToPath, pathToFileURL } from 'node:url';

registerHooks({
	resolve(specifier, context, next) {
		if ((specifier.startsWith('./') || specifier.startsWith('../')) && specifier.endsWith('.js') && context.parentURL) {
			const target = new URL(specifier, context.parentURL);
			const path = fileURLToPath(target);
			if (!existsSync(path)) {
				const ts = path.replace(/\/dist\//, '/src/').replace(/\.js$/, '.ts');
				if (existsSync(ts)) return next(pathToFileURL(ts).href, context);
			}
		}
		return next(specifier, context);
	},
});
