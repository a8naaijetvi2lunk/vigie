// Géométrie source commune. La CLI Tauri fournit les rendus natifs et le conteneur ICO.
import { readFileSync, writeFileSync, mkdirSync, mkdtempSync, copyFileSync, rmSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
const root = fileURLToPath(new URL('../', import.meta.url));
const source = readFileSync(path.join(root, 'src/assets/vigie-mark.svg'), 'utf8');
const output = path.join(root, 'src-tauri/icons/vigie');
mkdirSync(output, { recursive: true });
const input = path.join(output, 'app-icon.svg');
writeFileSync(input, source.replace('fill="none">', 'fill="none"><rect width="64" height="64" rx="14" fill="#F5F1EA"/>'), 'utf8');
const temporary = mkdtempSync(path.join(output, '.generate-'));
try {
  execFileSync(process.execPath, [path.join(root, 'node_modules/@tauri-apps/cli/tauri.js'), 'icon', input, '--output', temporary], { cwd: root, stdio: 'inherit' });
  for (const name of ['32x32.png', '128x128.png', '128x128@2x.png', 'icon.ico', 'icon.icns']) {
    copyFileSync(path.join(temporary, name), path.join(output, name));
  }
} finally {
  if (path.dirname(temporary) !== output || !path.basename(temporary).startsWith('.generate-')) throw new Error('Dossier temporaire inattendu');
  rmSync(temporary, { recursive: true });
}
console.log('Icônes Vigie : ' + output);
