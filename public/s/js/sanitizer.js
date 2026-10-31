/**
 * Simple Whitelist-based HTML Sanitizer
 * Protects against XSS by stripping disallowed tags and attributes.
 */
class Sanitizer {
    // F-009 Security: Removed most form elements (textarea, input, label, select, option)
    // to prevent phishing via user-generated content. 'button' is allowed because our own
    // UI injects <button> elements (e.g. load-full-comment-btn) inside comment content;
    // buttons cannot execute scripts and click handlers are class-based on a controlled container.
    // Style attribute is kept for admin-authored MOTD content.
    static ALLOWED_TAGS = ['span', 'img', 'a', 'br', 'b', 'i', 'strong', 'em', 'blockquote', 'pre', 'code', 'div', 'p', 'hr', 'ul', 'ol', 'li', 'svg', 'polyline', 'path', 'line', 'rect', 'circle', 'g', 'defs', 'symbol', 'use', 'polygon', 'ellipse', 'lineargradient', 'radialgradient', 'stop', 'clippath', 'mask', 'iframe', 'video', 'audio', 'button'];
    static ALLOWED_ATTRS = ['class', 'style', 'src', 'href', 'alt', 'title', 'target', 'width', 'height', 'placeholder', 'readonly', 'disabled', 'value', 'name', 'id', 'type', 'data-parent', 'data-id', 'data-username', 'xmlns', 'viewbox', 'fill', 'stroke', 'stroke-width', 'stroke-linecap', 'stroke-linejoin', 'points', 'x1', 'y1', 'x2', 'y2', 'd', 'transform', 'rx', 'ry', 'x', 'y', 'offset', 'stop-color', 'stop-opacity', 'fill-rule', 'clip-rule', 'cx', 'cy', 'r', 'fill-opacity', 'stroke-opacity', 'preserveaspectratio', 'vector-effect', 'pointer-events', 'allowfullscreen', 'frameborder', 'allow', 'referrerpolicy', 'rel', 'controls', 'loop', 'muted', 'playsinline', 'preload', 'tooltip', 'flow'];
    static DISALLOWED_URL_SCHEMES = ['javascript:', 'data:', 'vbscript:'];

    /**
     * Clean an HTML string
     * @param {string} html 
     * @returns {string} Sanitized HTML string
     */
    static clean(html) {
        if (!html) return '';
        const template = document.createElement('template');
        template.innerHTML = html;
        this.sanitizeNode(template.content);
        return template.innerHTML;
    }

    /**
     * Iteratively sanitize DOM nodes (prevents stack overflow)
     * @param {Node} root 
     */
    static sanitizeNode(root) {
        const stack = [root];
        while (stack.length > 0) {
            const current = stack.pop();
            const nodes = Array.from(current.childNodes);
            
            for (const node of nodes) {
                if (node.nodeType === Node.ELEMENT_NODE) {
                    const tagName = node.tagName.toLowerCase();
                    
                    if (!this.ALLOWED_TAGS.includes(tagName)) {
                        // If tag is not allowed, replace it with its text content
                        const text = document.createTextNode(node.textContent);
                        node.parentNode.replaceChild(text, node);
                    } else {
                        // Sanitize attributes
                        const attrs = Array.from(node.attributes);
                        for (const attr of attrs) {
                            const attrName = attr.name.toLowerCase();
                            
                            // Check if attribute is on whitelist or is a data- attribute
                            // data-on/data-args/data-arg bind site actions (csp-actions.js): never from user content
                            if ((!this.ALLOWED_ATTRS.includes(attrName) && !attrName.startsWith('data-')) || /^data-(on|args?)$/.test(attrName)) {
                                node.removeAttribute(attr.name);
                                continue;
                            }

                            // Special handling for URLs
                            if (attrName === 'href' || attrName === 'src') {
                                const val = attr.value.trim().toLowerCase();
                                if (this.DISALLOWED_URL_SCHEMES.some(scheme => val.startsWith(scheme))) {
                                    node.removeAttribute(attr.name);
                                }
                                // Iframes: allow YouTube and Vocaroo embed URLs
                                if (attrName === 'src' && tagName === 'iframe') {
                                    const isYouTube = val.startsWith('https://www.youtube.com/embed/');
                                    const isVocaroo = val.startsWith('https://vocaroo.com/embed/');
                                    if (!isYouTube && !isVocaroo) {
                                        node.removeAttribute(attr.name);
                                    }
                                }
                            }

                            // Special handling for style (extremely restrictive)
                            if (attrName === 'style') {
                                // Only allow specific safe CSS properties
                                const safeStyles = ['color', 'background', 'background-color', 'background-image', 'font-weight', 'font-style', 'text-decoration', 'vertical-align', 'height', 'width', 'display', 'fill', 'stroke', 'stroke-width', 'opacity', 'cursor', 'border', 'border-radius', 'padding', 'padding-left', 'padding-right', 'padding-top', 'padding-bottom', 'margin', 'margin-left', 'margin-right', 'margin-top', 'margin-bottom', 'position', 'top', 'left', 'right', 'bottom', 'z-index', 'flex', 'flex-direction', 'justify-content', 'align-items', 'gap'];
                                const styleParts = attr.value.split(';').filter(p => p.trim().length > 0);
                                const cleanStyles = styleParts.filter(part => {
                                    const prop = part.split(':')[0].trim().toLowerCase();
                                    if (!safeStyles.includes(prop)) return false;
                                    // F-009 Security: Strip url() from background/background-image
                                    // to prevent CSS-based tracking (e.g. background-image: url(https://evil.com/track))
                                    const val = part.split(':').slice(1).join(':').trim().toLowerCase();
                                    if ((prop === 'background-image' || prop === 'background') && /url\s*\(/i.test(val)) {
                                        return false;
                                    }
                                    return true;
                                });
                                if (cleanStyles.length > 0) {
                                    node.setAttribute(attr.name, cleanStyles.join('; '));
                                } else {
                                    node.removeAttribute(attr.name);
                                }
                            }
                        }
                        // Push to stack for iterative processing of children
                        stack.push(node);
                    }
                }
            }
        }
    }
}

// Global export
window.Sanitizer = Sanitizer;
