import { defineConfig } from "vite";

export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: process.env.TAURI_DEV_HOST || false,
    hmr: process.env.TAURI_DEV_HOST
      ? { protocol: "ws", host: process.env.TAURI_DEV_HOST, port: 1421 }
      : undefined,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  build: {
    target: "chrome105",
    minify: "esbuild",
    sourcemap: false,
    // 产物统一落在仓库根的「构建区」build/dist（与仓库根的 build/target 同级）。
    // 这里只写产物、不存源码，整个 build/ 都被 .gitignore 排除。
    outDir: "../build/dist",
    // 不清空 dist：产物使用 hash 文件名，旧的不会冲突。
    // 同时避免某些带 safe-delete 拦截的环境在构建时误触发批量删除保护。
    emptyOutDir: false,
  },
});
