/**
 * Join the defined parts with a separator, skipping the nullish, empty, and
 * false ones. `joinParts(["a", null, "b"])` is `"a · b"`.
 */
export function joinParts(
  parts: ReadonlyArray<string | null | undefined | false | "">,
  sep = " · ",
): string {
  return parts.filter(Boolean).join(sep);
}
