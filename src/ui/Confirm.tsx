import { signal } from "@preact/signals";
import { Modal } from "./Modal";

interface Req {
  title: string;
  body: string;
  confirm: string;
  danger?: boolean;
  resolve: (ok: boolean) => void;
}

const req = signal<Req | null>(null);

/** Ask a yes/no question in an accessible dialog. Resolves true on confirm. */
export function confirmDialog(o: { title: string; body: string; confirm?: string; danger?: boolean }): Promise<boolean> {
  return new Promise((resolve) => {
    req.value = { title: o.title, body: o.body, confirm: o.confirm ?? "OK", danger: o.danger, resolve };
  });
}

export function ConfirmHost() {
  const r = req.value;
  if (!r) return null;
  const done = (ok: boolean) => {
    req.value = null;
    r.resolve(ok);
  };
  return (
    <Modal
      title={r.title}
      onClose={() => done(false)}
      footer={
        <>
          <span class="spacer" />
          <button onClick={() => done(false)}>Cancel</button>
          <button class={r.danger ? "danger" : "primary"} data-autofocus onClick={() => done(true)}>{r.confirm}</button>
        </>
      }
    >
      <p class="dlg-lead">{r.body}</p>
    </Modal>
  );
}
