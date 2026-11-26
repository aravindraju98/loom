import { dismissToast, toasts } from "../state";

/** Stack of transient notifications, bottom-right. Errors stay until dismissed. */
export function Toasts() {
  const list = toasts.value;
  if (list.length === 0) return null;
  return (
    <div class="toasts" role="region" aria-label="Notifications">
      {list.map((t) => (
        <div key={t.id} class={`toast toast-${t.kind}`} role={t.kind === "error" ? "alert" : "status"}>
          <span class="toast-text">{t.text}</span>
          {t.action && (
            <button
              class="toast-action"
              onClick={() => {
                dismissToast(t.id);
                void t.action!.run();
              }}
            >
              {t.action.label}
            </button>
          )}
          <button class="toast-x" aria-label="Dismiss" onClick={() => dismissToast(t.id)}>×</button>
        </div>
      ))}
    </div>
  );
}
