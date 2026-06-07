// Copy third-party vendor files from node_modules to static/vendor/
// Run automatically as part of `npm run build`
import { cpSync, mkdirSync, readdirSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const uiRoot = join(__dirname, '..');
const staticVendor = join(uiRoot, '..', 'static', 'vendor');

mkdirSync(staticVendor, { recursive: true });

const copy = (src, dest) => {
  cpSync(src, dest, { recursive: true });
  console.log(`  ✓ ${dest.replace(uiRoot + '/..', '..')}`);
};

console.log('→ Copying vendor files to static/vendor/...');

// KaTeX: core + CSS + fonts + auto-render
copy(join(uiRoot, 'node_modules/katex/dist/katex.min.js'), join(staticVendor, 'katex.min.js'));
copy(join(uiRoot, 'node_modules/katex/dist/katex.min.css'), join(staticVendor, 'katex.min.css'));
copy(join(uiRoot, 'node_modules/katex/dist/fonts'), join(staticVendor, 'fonts'));
copy(join(uiRoot, 'node_modules/katex/dist/contrib/auto-render.min.js'), join(staticVendor, 'auto-render.min.js'));

// Prism: core is minified by esbuild in `npm run build`; autoloader + components follow
copy(join(uiRoot, 'node_modules/prismjs/plugins/autoloader/prism-autoloader.min.js'), join(staticVendor, 'prism-autoloader.min.js'));
// Copy all minified component files for autoloader
const componentsDir = join(uiRoot, 'node_modules/prismjs/components');
const prismComponentsDir = join(staticVendor, 'prism-components');
mkdirSync(prismComponentsDir, { recursive: true });
const componentFiles = readdirSync(componentsDir).filter(f => f.endsWith('.min.js'));
for (const f of componentFiles) {
  copy(join(componentsDir, f), join(prismComponentsDir, f));
}
console.log(`  ✓ prism-components/ (${componentFiles.length} language files)`);

// Prism themes (catppuccin) - source of truth in ui/src/vendor/
copy(join(uiRoot, 'src/vendor/prism-latte.css'), join(staticVendor, 'prism-latte.css'));
copy(join(uiRoot, 'src/vendor/prism-macchiato.css'), join(staticVendor, 'prism-macchiato.css'));

// Pseudocode.js CSS (JS is pre-built separately by `npm run build:pseudocode`)
copy(join(uiRoot, 'pseudocode.js/build/pseudocode.min.css'), join(uiRoot, '..', 'static/pseudocode.min.css'));

// Prettier: standalone + parsers
copy(join(uiRoot, 'node_modules/prettier/standalone.js'), join(staticVendor, 'prettier-standalone.js'));
const parsers = ['babel', 'html', 'markdown', 'yaml', 'typescript', 'postcss'];
for (const p of parsers) {
  copy(join(uiRoot, `node_modules/prettier/parser-${p}.js`), join(staticVendor, `prettier-parser-${p}.js`));
}

console.log('✓ Vendor files copied.');
