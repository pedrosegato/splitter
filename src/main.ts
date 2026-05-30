import { commands, events } from "./bindings";

const el = document.getElementById("app")!;

async function boot() {
  const res = await commands.listDevices();
  if (res.status === "ok") {
    el.textContent = `Splitter — ${res.data.length} audio devices`;
  } else {
    el.textContent = `error: ${res.error}`;
  }
  await events.peersChanged.listen((e) => {
    console.log("peers changed:", e.payload);
  });
}

boot().catch((err) => {
  el.textContent = `boot failed: ${err}`;
});
