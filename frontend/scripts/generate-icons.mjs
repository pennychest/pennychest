// Generates PWA icons from public/logo.png using sharp (available in devDeps).
// Run during Docker build before `vite build`.
import sharp from "sharp";
import { fileURLToPath } from "url";
import { dirname, resolve } from "path";

const __dirname = dirname(fileURLToPath(import.meta.url));
const src = resolve(__dirname, "../public/logo.png");
const out = resolve(__dirname, "../public");

for (const size of [192, 512]) {
  await sharp(src)
    .resize(size, size, { fit: "contain", background: { r: 0, g: 0, b: 0, alpha: 0 } })
    .png()
    .toFile(resolve(out, `pwa-${size}x${size}.png`));
  console.log(`Generated pwa-${size}x${size}.png`);
}
