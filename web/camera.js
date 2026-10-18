// Pan and zoom for the image viewer, without the page. The camera is the only state: the
// full-resolution pixel at the centre of the viewport (cx, cy) and the zoom z, in CSS pixels per
// full-resolution pixel. Every gesture computes a new camera from the one it started with, and
// every picture is placed from the camera and the region it shows, so repeated gestures can't
// drift and the viewport keeps its size whatever the pictures are.
//
// `view` is the viewport in CSS pixels ({ w, h }), `img` the image in full-resolution pixels
// ({ w, h }), `rect` a region of it ({ x, y, width, height }).

/** The zoom at which the whole image fits the viewport. */
export function fitZoom(view, img) {
  return Math.min(view.w / img.w, view.h / img.h);
}

/** The zoom limits: from the whole image to 4 CSS pixels per image pixel (or 4× the fit). */
export function limits(view, img) {
  const min = fitZoom(view, img);
  return { min, max: Math.max(4, min * 4) };
}

/** The camera kept inside the limits: an image narrower than the viewport stays centred. */
export function clamp(cam, view, img) {
  const { min, max } = limits(view, img);
  const z = Math.min(max, Math.max(min, cam.z));
  const axis = (c, span, size) => (span >= size ? size / 2 : Math.min(size - span / 2, Math.max(span / 2, c)));
  return { cx: axis(cam.cx, view.w / z, img.w), cy: axis(cam.cy, view.h / z, img.h), z };
}

/** The camera that shows `rect` whole, centred. */
export function showRect(rect, view, img) {
  const z = Math.min(view.w / rect.width, view.h / rect.height);
  return clamp({ cx: rect.x + rect.width / 2, cy: rect.y + rect.height / 2, z }, view, img);
}

/** Zoom by `f` keeping the image point under (px, py) (viewport CSS pixels) where it is. */
export function zoomAt(cam, f, px, py, view, img) {
  const ix = cam.cx + (px - view.w / 2) / cam.z;
  const iy = cam.cy + (py - view.h / 2) / cam.z;
  const { min, max } = limits(view, img);
  const z = Math.min(max, Math.max(min, cam.z * f));
  return clamp({ cx: ix - (px - view.w / 2) / z, cy: iy - (py - view.h / 2) / z, z }, view, img);
}

/** Move the picture by (dx, dy) CSS pixels. */
export function panBy(cam, dx, dy, view, img) {
  return clamp({ cx: cam.cx - dx / cam.z, cy: cam.cy - dy / cam.z, z: cam.z }, view, img);
}

/** The part of the image in the viewport, in full-resolution pixels. */
export function visible(cam, view, img) {
  const x0 = Math.max(0, cam.cx - view.w / 2 / cam.z);
  const y0 = Math.max(0, cam.cy - view.h / 2 / cam.z);
  const x1 = Math.min(img.w, cam.cx + view.w / 2 / cam.z);
  const y1 = Math.min(img.h, cam.cy + view.h / 2 / cam.z);
  return { x: x0, y: y0, width: Math.max(0, x1 - x0), height: Math.max(0, y1 - y0) };
}

/**
 * The region to draw for the camera: what is visible plus `margin` of it on each side (so a
 * small pan needs no new picture), in whole pixels inside the image, or undefined for the whole
 * image.
 */
export function regionFor(cam, view, img, margin = 0.1) {
  const v = visible(cam, view, img);
  const mx = v.width * margin;
  const my = v.height * margin;
  const x = Math.max(0, Math.floor(v.x - mx));
  const y = Math.max(0, Math.floor(v.y - my));
  const r = {
    x,
    y,
    width: Math.min(img.w, Math.ceil(v.x + v.width + mx)) - x,
    height: Math.min(img.h, Math.ceil(v.y + v.height + my)) - y,
  };
  return r.width >= img.w && r.height >= img.h ? undefined : r;
}

/**
 * Where a picture of `rect` goes: a CSS transform for an element `rect.width` × `rect.height`
 * CSS pixels with its origin at the top left.
 */
export function place(rect, cam, view) {
  return { x: (rect.x - cam.cx) * cam.z + view.w / 2, y: (rect.y - cam.cy) * cam.z + view.h / 2, s: cam.z };
}

/** Whether two cameras show the same thing (to within a hundredth of a pixel). */
export function same(a, b) {
  return Math.abs(a.cx - b.cx) < 0.01 && Math.abs(a.cy - b.cy) < 0.01 && Math.abs(a.z / b.z - 1) < 1e-4;
}
