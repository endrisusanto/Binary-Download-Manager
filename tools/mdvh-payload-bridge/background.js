const DEFAULT_RECEIVER_URL = "http://127.0.0.1:48991/import-mdvh";

chrome.runtime.onInstalled.addListener(async () => {
  const stored = await chrome.storage.local.get({
    receiverUrl: DEFAULT_RECEIVER_URL,
    interceptEnabled: true,
    lastPayload: null,
    lastResult: null,
  });
  await chrome.storage.local.set(stored);
});

chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  void (async () => {
    if (message?.type === "get-state") {
      sendResponse(await getState());
      return;
    }
    if (message?.type === "set-state") {
      const next = {};
      if (typeof message.receiverUrl === "string") next.receiverUrl = message.receiverUrl;
      if (typeof message.interceptEnabled === "boolean") next.interceptEnabled = message.interceptEnabled;
      await chrome.storage.local.set(next);
      sendResponse(await getState());
      return;
    }
    if (message?.type === "mdvh-payload") {
      const state = await getState();
      let cookiesStr = message.payload.cookies || "";
      try {
        const pageUrl = message.payload.pageUrl || sender.tab?.url;
        if (pageUrl) {
          const urlObj = new URL(pageUrl);
          // Query all cookies for this origin using privileged cookies API
          const cookies = await chrome.cookies.getAll({ url: urlObj.origin });
          if (cookies && cookies.length > 0) {
            cookiesStr = cookies.map(c => `${c.name}=${c.value}`).join("; ");
          }
        }
      } catch (err) {
        console.error("Failed to retrieve cookies via chrome.cookies API:", err);
      }

      const payload = {
        ...message.payload,
        cookies: cookiesStr,
        sourceTab: {
          id: sender.tab?.id,
          url: sender.tab?.url,
          title: sender.tab?.title,
        },
        capturedAt: new Date().toISOString(),
      };
      const result = await forwardPayload(state.receiverUrl, payload);
      await chrome.storage.local.set({ lastPayload: payload, lastResult: result });
      sendResponse(result);
      return;
    }
    sendResponse({ ok: false, error: "unknown message" });
  })();
  return true;
});

async function getState() {
  return chrome.storage.local.get({
    receiverUrl: DEFAULT_RECEIVER_URL,
    interceptEnabled: true,
    lastPayload: null,
    lastResult: null,
  });
}

async function forwardPayload(receiverUrl, payload) {
  try {
    const response = await fetch(receiverUrl, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload),
    });
    const text = await response.text();
    let body = text;
    try {
      body = JSON.parse(text);
    } catch {
      // keep raw response text
    }
    return {
      ok: response.ok,
      status: response.status,
      receiverUrl,
      body,
      at: new Date().toISOString(),
    };
  } catch (error) {
    return {
      ok: false,
      receiverUrl,
      error: String(error?.message || error),
      at: new Date().toISOString(),
    };
  }
}
