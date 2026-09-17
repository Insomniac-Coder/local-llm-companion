/** One line for the compact machine panel. A failed load used to print the
 * runtime's whole log excerpt there, a column of text that stayed until the
 * next successful load. The notification keeps the full message; the panel
 * shows the cause, and the complete text is on hover. */
export function loadErrorSummary(message: string | null | undefined, limit = 120): string {
  const text = (message ?? '').trim();
  if (!text) return 'The model could not be loaded.';
  // The log excerpt is appended after this marker; it is detail, not the cause.
  const cause = text.split(' Runtime diagnostic:')[0].replace(/\s+/g, ' ').trim();
  // Prefer the app's plain explanation when one follows the raw error.
  const explained = cause.match(/(The model file [^.]*\.|This runtime build [^.]*\.|A tensor in the model file [^.]*\.)/);
  const line = explained ? explained[1] : cause;
  return line.length > limit ? `${line.slice(0, limit - 1).trimEnd()}…` : line;
}
