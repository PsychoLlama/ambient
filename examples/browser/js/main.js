// The native side of `src/main.ab`: a function for each extern, by
// its name. A build of the package takes the ones its program reaches.

/** Writes a line where the page shows what the program says. */
export function said(line) {
  globalThis.document?.getElementById("said").append(`${line}\n`);
  return null;
}

/** Answers after a number of milliseconds: its extern is `suspending`. */
export function slept(milliseconds) {
  return new Promise((wake) => setTimeout(wake, milliseconds, null));
}
