import { useState } from 'react';
import { open } from '@tauri-apps/plugin-shell';
import { ask, message } from '@tauri-apps/plugin-dialog';
import { getVersion } from '@tauri-apps/api/app';

export const useUpdater = () => {
    const [checking, setChecking] = useState(false);

    const checkForUpdates = async (silent: boolean = false) => {
        if (checking) return;
        setChecking(true);

        try {
            // 1. Get current app version
            const currentVersion = await getVersion();

            // 2. Fetch latest release from GitHub API
            const response = await fetch('https://api.github.com/repos/shubham-pathak1/ciel/releases/latest');

            if (!response.ok) {
                throw new Error(`GitHub API error: ${response.statusText}`);
            }

            const latestRelease = await response.json();
            const latestVersion = latestRelease.tag_name.replace(/^v/, ''); // Remove 'v' prefix if present

            console.log(`[Updater] Current: ${currentVersion}, Latest: ${latestVersion}`);

            // 3. Compare versions (simple string comparison for now, or semantic versioning)
            const isUpdateAvailable = latestVersion !== currentVersion;

            if (isUpdateAvailable) {
                const confirmed = await ask(
                    `A new version of Ciel is available!\n\nVersion: ${latestRelease.tag_name}\nReleased: ${new Date(latestRelease.published_at).toLocaleDateString()}\n\n${latestRelease.body || "No release notes provided."}`,
                    {
                        title: 'Update Available',
                        kind: 'info',
                        okLabel: 'Go to Download Page',
                        cancelLabel: 'Later'
                    }
                );

                if (confirmed) {
                    // 4. Open GitHub releases in default browser
                    await open('https://github.com/shubham-pathak1/ciel/releases/latest');
                }
            } else if (!silent) {
                await message('You are running the latest version of Ciel.', { title: 'No Updates', kind: 'info' });
            }
        } catch (error) {
            console.error('[Updater] Check failed:', error);
            if (!silent) {
                await message(`Failed to check for updates.\n\n${String(error)}`, { title: 'Update Error', kind: 'error' });
            }
        } finally {
            setChecking(false);
        }
    };

    return { checkForUpdates, checking };
};
