const enabled = document.querySelector("#enabled");
const receiverUrl = document.querySelector("#receiver-url");
const status = document.querySelector("#status");
const last = document.querySelector("#last");

document.querySelector("#save").addEventListener("click", save);
document.querySelector("#refresh").addEventListener("click", render);

async function save() {
  await chrome.runtime.sendMessage({
    type: "set-state",
    interceptEnabled: enabled.checked,
    receiverUrl: receiverUrl.value.trim(),
  });
  await render();
}

async function render() {
  const state = await chrome.runtime.sendMessage({ type: "get-state" });
  enabled.checked = state.interceptEnabled !== false;
  receiverUrl.value = state.receiverUrl || "";
  const result = state.lastResult;
  status.textContent = result ? `Last send: ${result.ok ? "OK" : "FAILED"} ${result.status || ""}` : "No payload sent yet.";
  last.textContent = JSON.stringify(
    {
      lastResult: state.lastResult,
      lastPayload: summarizePayload(state.lastPayload),
    },
    null,
    2,
  );
}

function summarizePayload(payload) {
  if (!payload) return null;
  return {
    pageUrl: payload.pageUrl,
    selectedFiles: payload.selectedFiles,
    release: payload.release,
    capturedAt: payload.capturedAt,
  };
}

void render();
