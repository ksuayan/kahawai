/**
 * Built-ins the libraries use that older WebViews lack (the HiBy R4's
 * Chromium 91 has none of these: Object.hasOwn is Chromium 93, Array#at 92,
 * findLast 97, toSorted/toReversed 110). Each is defined only when missing, so
 * a current WebView (the desktop's included) is untouched. Imported first by
 * main.ts, before anything that might use them.
 */
function define<T extends object>(target: T, name: string, value: unknown): void {
  if (!(name in target)) {
    Object.defineProperty(target, name, { value, writable: true, configurable: true, enumerable: false });
  }
}

define(Object, "hasOwn", (obj: object, key: PropertyKey) => Object.prototype.hasOwnProperty.call(obj, key));

function at<T>(this: ArrayLike<T>, index: number): T | undefined {
  const n = Math.trunc(index) || 0;
  const i = n < 0 ? this.length + n : n;
  return i < 0 || i >= this.length ? undefined : this[i];
}
define(Array.prototype, "at", at);
define(String.prototype, "at", at);
define(Object.getPrototypeOf(Int8Array.prototype), "at", at);

define(Array.prototype, "findLast", function <T>(this: T[], pred: (v: T, i: number, a: T[]) => unknown, thisArg?: unknown) {
  for (let i = this.length - 1; i >= 0; i--) if (pred.call(thisArg, this[i]!, i, this)) return this[i];
  return undefined;
});
define(Array.prototype, "findLastIndex", function <T>(this: T[], pred: (v: T, i: number, a: T[]) => unknown, thisArg?: unknown) {
  for (let i = this.length - 1; i >= 0; i--) if (pred.call(thisArg, this[i]!, i, this)) return i;
  return -1;
});
define(Array.prototype, "toSorted", function <T>(this: T[], compare?: (a: T, b: T) => number) {
  return this.slice().sort(compare);
});
define(Array.prototype, "toReversed", function <T>(this: T[]) {
  return this.slice().reverse();
});

export {};
