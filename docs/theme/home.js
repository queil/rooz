// Landing page: widen the layout when the hero is present, and wire the install line's copy button.
(() => {
    if (!document.querySelector(".rooz-hero")) return;
    document.documentElement.classList.add("rooz-home");
    for (const button of document.querySelectorAll(".rooz-copy")) {
        button.addEventListener("click", async () => {
            try {
                await navigator.clipboard.writeText(button.dataset.copy);
                button.textContent = "Copied";
            } catch {
                button.textContent = "Press Ctrl+C";
                const code = button.parentElement.querySelector("code");
                window.getSelection().selectAllChildren(code);
            }
            setTimeout(() => (button.textContent = "Copy"), 1600);
        });
    }
})();
