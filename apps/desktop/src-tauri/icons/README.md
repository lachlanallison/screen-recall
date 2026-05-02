# Icons

Tauri needs PNG/ICO/ICNS icons here to bundle release installers.

For quick bootstrap, run:

```sh
pnpm --filter desktop tauri icon path/to/source-icon.png
```

from the repo root with any 1024×1024 PNG as input. That populates this folder with all required platform icons.

During `tauri dev` a missing icon is a warning, not an error, so you can start without this step.
