// Version badge in the top bar. The site workflow replaces "dev" with the release tag before building.
(() => {
    const version = "dev";
    const buttons = document.querySelector(".right-buttons");
    if (!buttons) return;
    const badge = document.createElement("a");
    badge.className = "rooz-version";
    badge.textContent = version;
    badge.title = "Release notes";
    badge.href = version === "dev"
        ? "https://github.com/queil/rooz/releases"
        : `https://github.com/queil/rooz/releases/tag/${version}`;
    buttons.prepend(badge);
})();
