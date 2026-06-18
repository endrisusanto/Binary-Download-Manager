(function () {
  // Ponytail: Run directly in the MAIN world. No injection wrapper or textContent hacks.
  window.__raon_spy_reports = [];

  const recordEvent = (type, title, data) => {
    const item = {
      timestamp: new Date().toISOString(),
      type,
      title,
      data
    };
    window.__raon_spy_reports.push(item);
    
    // Update badge count
    const badge = document.querySelector('#raon-spy-badge');
    if (badge) {
      badge.textContent = window.__raon_spy_reports.length;
    }
  };

  const log = (title, details, type = 'info') => {
    const colors = {
      info: '#3B82F6',
      success: '#10B981',
      warning: '#F59E0B',
      error: '#EF4444'
    };
    const color = colors[type] || colors.info;
    console.groupCollapsed(
      `%c[RAON SPY] ${title}`,
      `color: ${color}; font-weight: bold; font-size: 11px; padding: 2px 4px; border-radius: 3px; background: rgba(0,0,0,0.05);`
    );
    console.log('Timestamp:', new Date().toLocaleTimeString());
    if (details) {
      console.dir(details);
    }
    console.groupEnd();
    
    recordEvent(type, title, details);
  };

  log('RAONK Downloader Spy Active', 'Monitoring network requests, XMLHttpRequests, Fetch, WebSockets, and Forms.');

  // 1. Intercept XMLHttpRequest
  const OriginalXHR = window.XMLHttpRequest;
  function SpyXHR() {
    const xhr = new OriginalXHR();
    const send = xhr.send;
    const open = xhr.open;
    let method = '';
    let url = '';
    let requestHeaders = {};

    xhr.open = function(m, u, ...args) {
      method = m;
      url = u;
      return open.apply(xhr, [m, u, ...args]);
    };

    const setRequestHeader = xhr.setRequestHeader;
    xhr.setRequestHeader = function(header, value) {
      requestHeaders[header] = value;
      return setRequestHeader.apply(xhr, [header, value]);
    };

    xhr.send = function(body) {
      const isTarget = url.includes('127.0.0.1') || url.includes('localhost') || url.includes('raonk') || url.includes('kupload') || url.includes('Download') || url.includes('srBinary');
      
      if (isTarget) {
        const reqId = Math.random().toString(36).substring(7);
        log(`XHR Request [${reqId}]: ${method} ${url}`, {
          url,
          method,
          headers: requestHeaders,
          body: body
        }, 'warning');

        xhr.addEventListener('load', () => {
          log(`XHR Response [${reqId}]: ${url} [${xhr.status}]`, {
            status: xhr.status,
            responseText: xhr.responseText
          }, xhr.status >= 200 && xhr.status < 300 ? 'success' : 'error');
        });
      }
      return send.apply(xhr, [body]);
    };

    return xhr;
  }
  SpyXHR.prototype = OriginalXHR.prototype;
  Object.assign(SpyXHR, OriginalXHR);
  window.XMLHttpRequest = SpyXHR;

  // 2. Intercept Fetch
  const originalFetch = window.fetch;
  window.fetch = async function(resource, init) {
    const url = typeof resource === 'string' ? resource : (resource.url || '');
    const method = (init && init.method) || 'GET';
    const isTarget = url.includes('127.0.0.1') || url.includes('localhost') || url.includes('raonk') || url.includes('kupload') || url.includes('Download') || url.includes('srBinary');

    if (isTarget) {
      log(`Fetch Request: ${method} ${url}`, {
        url,
        method,
        headers: (init && init.headers) || {},
        body: (init && init.body) || null
      }, 'warning');
    }

    try {
      const response = await originalFetch(resource, init);
      if (isTarget) {
        const clone = response.clone();
        clone.text().then(text => {
          log(`Fetch Response: ${url} [${response.status}]`, {
            status: response.status,
            body: text
          }, response.status >= 200 && response.status < 300 ? 'success' : 'error');
        });
      }
      return response;
    } catch (err) {
      if (isTarget) {
        log(`Fetch Failed: ${url}`, err, 'error');
      }
      throw err;
    }
  };

  // 3. Intercept WebSockets
  const OriginalWebSocket = window.WebSocket;
  window.WebSocket = function(url, protocols) {
    log(`WebSocket Connection Attempt: ${url}`, { url, protocols }, 'warning');
    const ws = new OriginalWebSocket(url, protocols);
    
    ws.addEventListener('open', () => {
      log(`WebSocket Opened: ${url}`, null, 'success');
    });

    ws.addEventListener('message', (event) => {
      log(`WebSocket Message Received from ${url}`, event.data, 'info');
    });

    const originalSend = ws.send;
    ws.send = function(data) {
      log(`WebSocket Message Sent to ${url}`, data, 'info');
      return originalSend.apply(ws, [data]);
    };

    ws.addEventListener('close', (event) => {
      log(`WebSocket Closed: ${url}`, { code: event.code, reason: event.reason }, 'error');
    });

    return ws;
  };
  window.WebSocket.prototype = OriginalWebSocket.prototype;

  // 4. Intercept Form Submission
  window.addEventListener('submit', (event) => {
    const form = event.target;
    const action = form.action || '';
    const method = form.method || 'GET';
    
    const formData = {};
    const inputs = form.querySelectorAll('input, select, textarea');
    inputs.forEach(input => {
      if (input.name) {
        if (input.type === 'checkbox' || input.type === 'radio') {
          if (input.checked) formData[input.name] = input.value;
        } else {
          formData[input.name] = input.value;
        }
      }
    });

    log(`Form Submitted to ${action} [${method}]`, {
      action,
      method,
      formData
    }, 'warning');
  }, true);

  // 5. Periodically inspect global RAONK objects if any
  setInterval(() => {
    const globals = ['RAONK', 'RAONK_Download', 'RAONK_Upload', 'KUpload', 'raonk_download', 'kupload'];
    globals.forEach(g => {
      if (window[g] && !window['__spy_logged_' + g]) {
        window['__spy_logged_' + g] = true;
        log(`Detected Global Object: window.${g}`, window[g], 'success');
      }
    });
  }, 2000);

  // 6. Setup Float UI
  const addUI = () => {
    if (document.getElementById('raon-spy-btn')) return;

    const style = document.createElement('style');
    style.id = 'raon-spy-styles';
    style.textContent = `
      #raon-spy-btn {
        position: fixed;
        bottom: 24px;
        right: 24px;
        z-index: 2147483647;
        background: rgba(16, 185, 129, 0.85);
        backdrop-filter: blur(8px);
        -webkit-backdrop-filter: blur(8px);
        color: #ffffff;
        border: 1px solid rgba(255, 255, 255, 0.2);
        padding: 12px 20px;
        border-radius: 30px;
        font-family: system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
        font-size: 13px;
        font-weight: 600;
        cursor: pointer;
        box-shadow: 0 10px 25px rgba(0, 0, 0, 0.3);
        transition: all 0.2s cubic-bezier(0.4, 0, 0.2, 1);
        display: flex;
        align-items: center;
        gap: 10px;
      }
      #raon-spy-btn:hover {
        background: rgba(16, 185, 129, 1);
        transform: translateY(-2px);
        box-shadow: 0 12px 30px rgba(0, 0, 0, 0.4);
      }
      #raon-spy-btn:active {
        transform: translateY(0);
      }
      #raon-spy-badge {
        background: #ffffff;
        color: #10b981;
        padding: 2px 7px;
        border-radius: 10px;
        font-size: 11px;
        font-weight: bold;
        display: inline-block;
        box-shadow: 0 2px 4px rgba(0,0,0,0.1);
      }
      .raon-spy-toast {
        position: fixed;
        bottom: 80px;
        right: 24px;
        z-index: 2147483647;
        background: #111827;
        color: #ffffff;
        padding: 8px 16px;
        border-radius: 20px;
        font-family: system-ui, sans-serif;
        font-size: 12px;
        box-shadow: 0 4px 12px rgba(0,0,0,0.15);
        border: 1px solid rgba(255,255,255,0.1);
        transition: opacity 0.3s ease;
      }
      .raon-spy-modal-overlay {
        position: fixed;
        top: 0;
        left: 0;
        width: 100vw;
        height: 100vh;
        z-index: 2147483646;
        background: rgba(0, 0, 0, 0.65);
        backdrop-filter: blur(4px);
        display: flex;
        align-items: center;
        justify-content: center;
        font-family: system-ui, -apple-system, sans-serif;
      }
      .raon-spy-modal {
        background: #1f2937;
        color: #f3f4f6;
        width: 90%;
        max-width: 650px;
        height: 80%;
        max-height: 500px;
        border-radius: 12px;
        border: 1px solid rgba(255, 255, 255, 0.1);
        box-shadow: 0 20px 25px -5px rgba(0, 0, 0, 0.3);
        display: flex;
        flex-direction: column;
        overflow: hidden;
      }
      .raon-spy-modal-header {
        padding: 16px 20px;
        border-bottom: 1px solid rgba(255, 255, 255, 0.1);
        display: flex;
        justify-content: space-between;
        align-items: center;
        font-weight: 600;
        font-size: 15px;
      }
      .raon-spy-modal-body {
        flex: 1;
        padding: 20px;
        display: flex;
        flex-direction: column;
      }
      .raon-spy-modal-textarea {
        flex: 1;
        width: 100%;
        height: 100%;
        background: #111827;
        color: #34d399;
        font-family: "Fira Code", Monaco, Consolas, "Courier New", monospace;
        font-size: 11px;
        padding: 12px;
        border-radius: 8px;
        border: 1px solid rgba(255, 255, 255, 0.15);
        resize: none;
        outline: none;
      }
      .raon-spy-modal-footer {
        padding: 16px 20px;
        border-top: 1px solid rgba(255, 255, 255, 0.1);
        display: flex;
        justify-content: flex-end;
        gap: 12px;
      }
      .raon-spy-modal-btn-action {
        background: #10b981;
        color: white;
        border: none;
        padding: 8px 16px;
        border-radius: 6px;
        cursor: pointer;
        font-size: 13px;
        font-weight: 600;
        transition: background-color 0.2s;
      }
      .raon-spy-modal-btn-action:hover {
        background: #059669;
      }
      .raon-spy-modal-btn-close {
        background: #4b5563;
        color: white;
        border: none;
        padding: 8px 16px;
        border-radius: 6px;
        cursor: pointer;
        font-size: 13px;
        font-weight: 600;
        transition: background-color 0.2s;
      }
      .raon-spy-modal-btn-close:hover {
        background: #374151;
      }
    `;
    document.head.appendChild(style);

    const showToast = (message, duration = 3000) => {
      const existing = document.querySelector('.raon-spy-toast');
      if (existing) existing.remove();
      const toast = document.createElement('div');
      toast.className = 'raon-spy-toast';
      toast.textContent = message;
      document.body.appendChild(toast);
      setTimeout(() => {
        toast.style.opacity = '0';
        setTimeout(() => toast.remove(), 300);
      }, duration);
    };

    const showModal = (content) => {
      const existing = document.querySelector('.raon-spy-modal-overlay');
      if (existing) existing.remove();
      
      const overlay = document.createElement('div');
      overlay.className = 'raon-spy-modal-overlay';
      overlay.innerHTML = `
        <div class="raon-spy-modal">
          <div class="raon-spy-modal-header">
            <span>RAONK Downloader Spy Report Viewer</span>
            <span style="font-size: 12px; color: #9ca3af;">\${window.__raon_spy_reports.length} events captured</span>
          </div>
          <div class="raon-spy-modal-body">
            <p style="margin: 0 0 12px 0; font-size: 12px; color: #9ca3af;">If the JSON file did not download automatically due to page security policies, you can copy the data below or use the "Copy to Clipboard" button.</p>
            <textarea class="raon-spy-modal-textarea" readonly></textarea>
          </div>
          <div class="raon-spy-modal-footer">
            <button class="raon-spy-modal-btn-action">📋 Copy to Clipboard</button>
            <button class="raon-spy-modal-btn-close">Close</button>
          </div>
        </div>
      `;
      
      overlay.querySelector('.raon-spy-modal-textarea').value = content;
      
      const copyBtn = overlay.querySelector('.raon-spy-modal-btn-action');
      copyBtn.addEventListener('click', () => {
        navigator.clipboard.writeText(content).then(() => {
          showToast('Copied report to clipboard!');
          copyBtn.textContent = '✅ Copied!';
          setTimeout(() => copyBtn.innerHTML = '📋 Copy to Clipboard', 2000);
        }).catch(err => {
          showToast('Failed to copy automatically: ' + err);
        });
      });
      
      const closeBtn = overlay.querySelector('.raon-spy-modal-btn-close');
      closeBtn.addEventListener('click', () => overlay.remove());
      
      overlay.addEventListener('click', (e) => {
        if (e.target === overlay) overlay.remove();
      });
      
      document.body.appendChild(overlay);
    };

    const btn = document.createElement('button');
    btn.id = 'raon-spy-btn';
    btn.innerHTML = `<span>💾 Download Spy Report</span><span id="raon-spy-badge">\${window.__raon_spy_reports.length}</span>`;
    
    btn.addEventListener('click', () => {
      const jsonStr = JSON.stringify(window.__raon_spy_reports, null, 2);
      
      // 1. Attempt file download
      try {
        const blob = new Blob([jsonStr], { type: 'application/json' });
        const url = URL.createObjectURL(blob);
        const a = document.createElement('a');
        a.href = url;
        a.download = 'raon-spy-report-' + Date.now() + '.json';
        document.body.appendChild(a);
        a.click();
        a.remove();
        URL.revokeObjectURL(url);
      } catch (err) {
        console.error('File download failed:', err);
      }
      
      // 2. Attempt Clipboard Copy
      navigator.clipboard.writeText(jsonStr).then(() => {
        showToast('Copied spy report JSON to clipboard!');
      }).catch(err => {
        console.warn('Clipboard write failed:', err);
      });
      
      // 3. Show Modal
      showModal(jsonStr);
    });

    document.body.appendChild(btn);
  };

  if (document.body) {
    addUI();
  } else {
    window.addEventListener('DOMContentLoaded', addUI);
  }
})();
