import { readFileSync } from 'fs';
import { join, resolve } from 'path';

const localesDir = join(resolve(), 'locales');

function loadLocale(lang) {
  try {
    return JSON.parse(readFileSync(join(localesDir, `${lang}.json`), 'utf8'));
  } catch (e) {
    console.warn(`[i18n] Failed to load locale "${lang}":`, e.message);
    return {};
  }
}

/**
 * Retrieves a nested value from an object using dot-notation.
 * e.g. get(obj, 'nav.login') => obj.nav.login
 */
function deepGet(obj, key) {
  return key.split('.').reduce((o, k) => o?.[k], obj);
}

/**
 * Creates an i18n instance for the given language.
 * Falls back to English for any missing keys.
 * No cache: always reads fresh from disk so locale changes
 * take effect without restarting the server.
 *
 * @param {string} lang - language code, e.g. 'de' or 'en'
 * @returns {{ t: Function, lang: string }}
 */
export function createI18n(lang = 'en') {
  const primary = loadLocale(lang);
  const fallback = lang !== 'en' ? loadLocale('en') : {};

  /**
   * Translate a dot-notation key.
   * Returns the translated string, falling back to the English string,
   * then to the raw key if nothing else matches.
   * Supports variable interpolation using {key} syntax.
   *
   * @param {string} key - e.g. 'nav.login'
   * @param {Object} [data] - optional variables to interpolate
   * @returns {string}
   */
  function t(key, data = {}) {
    let str = deepGet(primary, key) ?? deepGet(fallback, key) ?? key;
    if (typeof str !== 'string') return str;
    Object.keys(data).forEach(k => {
      const escapedK = k.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
      str = str.replace(new RegExp(`\\{${escapedK}\\}`, 'g'), data[k]);
    });
    return str;
  }

  return { t, lang };
}
