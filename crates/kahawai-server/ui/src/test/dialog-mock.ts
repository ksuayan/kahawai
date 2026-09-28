// Controllable stand-in for `@tauri-apps/plugin-dialog`'s `open()`, which the
// player's shared tauri-mock does not cover (the player has no directory
// picker). A test sets `dialog.nextPath` before triggering the picker.

class DialogMock {
  nextPath: string | null | undefined = undefined;

  reset(): void {
    this.nextPath = undefined;
  }

  async open(): Promise<string | null> {
    return this.nextPath ?? null;
  }
}

export const dialog = new DialogMock();
