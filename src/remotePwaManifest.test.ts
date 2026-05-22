import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { inflateSync } from "node:zlib";
import { describe, expect, it } from "vitest";

type ManifestIcon = {
  src?: string;
  sizes?: string;
  type?: string;
  purpose?: string;
};

type RgbaPixel = {
  r: number;
  g: number;
  b: number;
  a: number;
};

type PngPixels = {
  width: number;
  height: number;
  pixelAt: (x: number, y: number) => RgbaPixel;
};

const paethPredictor = (left: number, above: number, upperLeft: number) => {
  const estimate = left + above - upperLeft;
  const distanceLeft = Math.abs(estimate - left);
  const distanceAbove = Math.abs(estimate - above);
  const distanceUpperLeft = Math.abs(estimate - upperLeft);
  if (distanceLeft <= distanceAbove && distanceLeft <= distanceUpperLeft) return left;
  if (distanceAbove <= distanceUpperLeft) return above;
  return upperLeft;
};

const readPngPixels = (png: Buffer): PngPixels => {
  const signature = png.subarray(0, 8).toString("hex");
  expect(signature).toBe("89504e470d0a1a0a");

  let offset = 8;
  let width = 0;
  let height = 0;
  let colorType = 0;
  let bitDepth = 0;
  const idatChunks: Buffer[] = [];

  while (offset < png.length) {
    const length = png.readUInt32BE(offset);
    const type = png.subarray(offset + 4, offset + 8).toString("ascii");
    const dataStart = offset + 8;
    const dataEnd = dataStart + length;
    const data = png.subarray(dataStart, dataEnd);

    if (type === "IHDR") {
      width = data.readUInt32BE(0);
      height = data.readUInt32BE(4);
      bitDepth = data.readUInt8(8);
      colorType = data.readUInt8(9);
    } else if (type === "IDAT") {
      idatChunks.push(data);
    } else if (type === "IEND") {
      break;
    }

    offset = dataEnd + 4;
  }

  expect(bitDepth).toBe(8);
  expect([2, 6]).toContain(colorType);

  const bytesPerPixel = colorType === 6 ? 4 : 3;
  const stride = width * bytesPerPixel;
  const inflated = inflateSync(Buffer.concat(idatChunks));
  const pixels = Buffer.alloc(height * stride);

  for (let y = 0; y < height; y += 1) {
    const scanlineOffset = y * (stride + 1);
    const filterType = inflated[scanlineOffset];
    const scanline = inflated.subarray(scanlineOffset + 1, scanlineOffset + 1 + stride);
    expect([0, 1, 2, 3, 4]).toContain(filterType);

    for (let x = 0; x < stride; x += 1) {
      const left = x >= bytesPerPixel ? pixels[y * stride + x - bytesPerPixel] : 0;
      const above = y > 0 ? pixels[(y - 1) * stride + x] : 0;
      const upperLeft = y > 0 && x >= bytesPerPixel ? pixels[(y - 1) * stride + x - bytesPerPixel] : 0;
      const source = scanline[x];
      let value = source;

      if (filterType === 1) value = source + left;
      else if (filterType === 2) value = source + above;
      else if (filterType === 3) value = source + Math.floor((left + above) / 2);
      else if (filterType === 4) value = source + paethPredictor(left, above, upperLeft);

      pixels[y * stride + x] = value & 0xff;
    }
  }

  const pixelAt = (x: number, y: number): RgbaPixel => {
    const pixelOffset = y * stride + x * bytesPerPixel;
    return {
      r: pixels[pixelOffset],
      g: pixels[pixelOffset + 1],
      b: pixels[pixelOffset + 2],
      a: colorType === 6 ? pixels[pixelOffset + 3] : 255,
    };
  };

  return { width, height, pixelAt };
};

const measureNonWhiteBounds = (icon: PngPixels) => {
  let minX = icon.width;
  let minY = icon.height;
  let maxX = -1;
  let maxY = -1;

  for (let y = 0; y < icon.height; y += 1) {
    for (let x = 0; x < icon.width; x += 1) {
      const pixel = icon.pixelAt(x, y);
      const isWhite = pixel.r === 255 && pixel.g === 255 && pixel.b === 255 && pixel.a === 255;
      if (!isWhite) {
        minX = Math.min(minX, x);
        minY = Math.min(minY, y);
        maxX = Math.max(maxX, x);
        maxY = Math.max(maxY, y);
      }
    }
  }

  expect(maxX).toBeGreaterThanOrEqual(0);
  expect(maxY).toBeGreaterThanOrEqual(0);

  return {
    width: maxX - minX + 1,
    height: maxY - minY + 1,
  };
};

describe("remote PWA manifest", () => {
  it("advertises a dedicated maskable PNG icon for Android install surfaces", () => {
    const publicDir = join(process.cwd(), "public");
    const manifest = JSON.parse(
      readFileSync(join(publicDir, "manifest.webmanifest"), "utf8"),
    ) as { icons?: ManifestIcon[] };

    const maskableIcon = manifest.icons?.find((icon) =>
      icon.purpose?.split(/\s+/).includes("maskable"),
    );

    expect(maskableIcon).toMatchObject({
      src: "/icon-maskable.png",
      sizes: "512x512",
      type: "image/png",
      purpose: "maskable",
    });
    expect(existsSync(join(publicDir, maskableIcon?.src?.replace(/^\//, "") ?? ""))).toBe(true);
  });

  it("uses an opaque white background for the maskable mobile icon", () => {
    const icon = readPngPixels(readFileSync(join(process.cwd(), "public", "icon-maskable.png")));

    expect(icon.width).toBe(512);
    expect(icon.height).toBe(512);
    expect(icon.pixelAt(0, 0)).toEqual({ r: 255, g: 255, b: 255, a: 255 });
    expect(icon.pixelAt(511, 0)).toEqual({ r: 255, g: 255, b: 255, a: 255 });
    expect(icon.pixelAt(0, 511)).toEqual({ r: 255, g: 255, b: 255, a: 255 });
    expect(icon.pixelAt(511, 511)).toEqual({ r: 255, g: 255, b: 255, a: 255 });
  });

  it("keeps the logo inside the maskable icon safe area", () => {
    const icon = readPngPixels(readFileSync(join(process.cwd(), "public", "icon-maskable.png")));
    const bounds = measureNonWhiteBounds(icon);

    expect(Math.max(bounds.width, bounds.height)).toBeLessThanOrEqual(384);
  });
});
