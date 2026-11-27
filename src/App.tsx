import { RestoreDialog } from "./RestoreDialog";
import { restoreReq } from "./state";
import { ConfirmHost } from "./ui/Confirm";
import { Toasts } from "./ui/Toasts";
import { Workbench } from "./workbench/Workbench";

export function App() {
  return (
    <>
      <Workbench />
      {restoreReq.value && <RestoreDialog />}
      <ConfirmHost />
      <Toasts />
    </>
  );
}
