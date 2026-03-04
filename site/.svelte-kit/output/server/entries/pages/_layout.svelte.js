import { e as ensure_array_like, a as attr, b as attr_class, c as escape_html } from "../../chunks/index.js";
import { p as page } from "../../chunks/index2.js";
function _layout($$renderer, $$props) {
  $$renderer.component(($$renderer2) => {
    let { children } = $$props;
    const navLinks = [
      { href: "/", label: "Home" },
      { href: "/install", label: "Install" },
      { href: "/compare", label: "Compare" },
      { href: "/about", label: "About" }
    ];
    function isActive(href) {
      if (href === "/") return page.url.pathname === "/";
      return page.url.pathname.startsWith(href);
    }
    $$renderer2.push(`<div class="app svelte-12qhfyh"><header class="site-header svelte-12qhfyh"><div class="container header-inner svelte-12qhfyh"><a href="/" class="logo svelte-12qhfyh"><span class="logo-text svelte-12qhfyh">conary</span></a> <nav aria-label="Main navigation"><ul class="nav-links svelte-12qhfyh"><!--[-->`);
    const each_array = ensure_array_like(navLinks);
    for (let $$index = 0, $$length = each_array.length; $$index < $$length; $$index++) {
      let link = each_array[$$index];
      $$renderer2.push(`<li><a${attr("href", link.href)}${attr_class("svelte-12qhfyh", void 0, { "active": isActive(link.href) })}>${escape_html(link.label)}</a></li>`);
    }
    $$renderer2.push(`<!--]--> <li><a href="https://packages.conary.io" class="nav-external svelte-12qhfyh">Packages</a></li></ul></nav></div></header> <main class="svelte-12qhfyh">`);
    children($$renderer2);
    $$renderer2.push(`<!----></main> <footer class="site-footer svelte-12qhfyh"><div class="container footer-inner svelte-12qhfyh"><div class="footer-links svelte-12qhfyh"><a href="/" class="svelte-12qhfyh">Home</a> <a href="/install" class="svelte-12qhfyh">Install</a> <a href="/compare" class="svelte-12qhfyh">Compare</a> <a href="/about" class="svelte-12qhfyh">About</a> <a href="https://packages.conary.io" class="svelte-12qhfyh">Packages</a> <a href="https://github.com/ConaryLabs/Conary" target="_blank" rel="noopener noreferrer" class="svelte-12qhfyh">GitHub</a></div> <div class="footer-bottom svelte-12qhfyh"><span class="footer-prompt svelte-12qhfyh">$</span> <span class="footer-text svelte-12qhfyh">conary -- the cross-distribution package manager</span></div></div></footer></div>`);
  });
}
export {
  _layout as default
};
