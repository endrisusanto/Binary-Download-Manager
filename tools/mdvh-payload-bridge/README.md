# MDVH Payload Bridge

Chrome/Edge unpacked extension untuk phase pertama Binary Download Manager.

Fungsinya:

- Intercept klik tombol `Download` di MDVH.
- Mencegah RAON dialog terbuka saat intercept aktif.
- Mengambil metadata checkbox terpilih dari hidden input `selectFileMeta`.
- Mengirim payload ke receiver lokal `mdvh-agent-probe --listen-payload`.

## Cara Pakai

1. Jalankan receiver:

   ```cmd
   mdvh-agent-probe.exe ^
     --listen-payload ^
     --output-dir payloads ^
     --listen-port 48991
   ```

2. Buka Chrome/Edge `chrome://extensions` atau `edge://extensions`.
3. Aktifkan `Developer mode`.
4. Klik `Load unpacked`.
5. Pilih folder `tools/mdvh-payload-bridge`.
6. Buka halaman MDVH, centang file, klik `Download`.
7. Receiver akan menyimpan:
   - `payloads/latest-mdvh-payload.json`
   - `payloads/mdvh-payload-<timestamp>.json`

Matikan toggle `Intercept` jika ingin kembali membuka RAON dialog normal.
