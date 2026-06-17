(function () {
  const DOWNLOAD_SELECTOR = "#btnFileDownload";

  document.addEventListener(
    "click",
    async (event) => {
      const target = event.target instanceof Element ? event.target.closest(DOWNLOAD_SELECTOR) : null;
      if (!target) return;

      const state = await chrome.runtime.sendMessage({ type: "get-state" });
      if (!state.interceptEnabled) return;

      const payload = collectPayload(target);
      event.preventDefault();
      event.stopPropagation();
      event.stopImmediatePropagation();

      const result = await chrome.runtime.sendMessage({ type: "mdvh-payload", payload });
      showBridgeToast(result.ok ? "Sent to Binary Download Manager" : `Bridge failed: ${result.error || result.status}`);
    },
    true,
  );

  function collectPayload(target) {
    const selectedFiles = collectSelectedFiles();
    return {
      kind: "mdvh-selected-binaries",
      pageUrl: location.href,
      pageTitle: document.title,
      button: {
        id: target.id,
        text: target.textContent?.trim() || "",
      },
      release: collectReleaseFields(),
      selectedFiles,
      rawCheckedInputs: collectCheckedInputs(),
      userAgent: navigator.userAgent,
    };
  }

  function collectSelectedFiles() {
    const inputs = Array.from(document.querySelectorAll("input"));
    const files = [];
    for (let index = 0; index < inputs.length; index += 1) {
      const input = inputs[index];
      if (input.name !== "selectFile" || !input.checked) continue;
      const item = {
        checkboxClass: input.className || "",
        checkboxId: input.id || "",
      };
      for (let lookahead = index + 1; lookahead < Math.min(inputs.length, index + 10); lookahead += 1) {
        const sibling = inputs[lookahead];
        if (sibling.name === "selectFileMeta") {
          const [stamp, fileName, fileType, serverPath, size] = String(sibling.value || "").split("*");
          Object.assign(item, { stamp, fileName, fileType, serverPath, size });
        } else if (sibling.name === "selectFileBinaryId") {
          item.binaryId = sibling.value;
        } else if (sibling.name === "selectFileId") {
          item.fileId = sibling.value;
          break;
        }
      }
      files.push(item);
    }
    return files;
  }

  function collectReleaseFields() {
    const names = [
      "releaseInfoVo.releaseId",
      "releaseInfoVo.releaseDetailId",
      "releaseInfoVo.cscOpVersionInfoId",
      "releaseInfoVo.codeOpVersionInfoId",
      "releaseInfoVo.bbOpVersionInfoId",
      "approvalId",
      "releaseInfoVo.isOpenBinary",
      "raonkFlag",
    ];
    const values = {};
    for (const name of names) {
      const input = document.querySelector(`input[name="${cssEscape(name)}"]`);
      if (input) values[name] = input.value;
    }
    return values;
  }

  function collectCheckedInputs() {
    return Array.from(document.querySelectorAll("input[type='checkbox']:checked")).map((input) => ({
      name: input.name,
      id: input.id,
      className: input.className,
      value: input.value,
    }));
  }

  function cssEscape(value) {
    if (window.CSS?.escape) return CSS.escape(value);
    return String(value).replace(/"/g, '\\"');
  }

  function showBridgeToast(text) {
    const existing = document.querySelector("#mdvh-payload-bridge-toast");
    if (existing) existing.remove();
    const toast = document.createElement("div");
    toast.id = "mdvh-payload-bridge-toast";
    toast.textContent = text;
    Object.assign(toast.style, {
      position: "fixed",
      right: "16px",
      bottom: "16px",
      zIndex: "2147483647",
      background: "#111827",
      color: "#ffffff",
      padding: "10px 12px",
      borderRadius: "6px",
      font: "12px system-ui, sans-serif",
      boxShadow: "0 8px 24px rgba(0,0,0,.25)",
    });
    document.documentElement.appendChild(toast);
    setTimeout(() => toast.remove(), 3500);
  }
})();
