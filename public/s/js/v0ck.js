(function() {
const tpl_player = (svg, size) => `<div class="v0ck_player_controls">
  <div class="v0ck_progress">
    <div class="v0ck_progress_buffered"></div>
    <div class="v0ck_progress_filled"></div>
    <div class="v0ck_seek_marker"></div>
  </div>
  <button class="v0ck_player_button v0ck_tplay v0ck_toggle" title="Play" tabindex="-1">
    <svg style="width: 20px; height: 20px;">
      <use id="v0ck_svg_play" href="${svg}#play"></use>
      <use id="v0ck_svg_pause" class="v0ck_hidden" href="${svg}#pause"></use>
    </svg>
  </button>
  <div class="v0ck_volume_group">
  <button class="v0ck_player_button v0ck_volume" tabindex="-1">
    <svg style="width: 20px; height: 20px;">
      <use id="v0ck_svg_volume_full" href="${svg}#volume_full"></use>
      <use id="v0ck_svg_volume_mid" class="v0ck_hidden" href="${svg}#volume_mid"></use>
      <use id="v0ck_svg_volume_mute" class="v0ck_hidden" href="${svg}#volume_mute"></use>
    </svg>
  </button>
  <input type="range" name="volume" min="0" max="1" step="0.01" value="1" tabindex="-1" />
  </div>
  <button class="v0ck_player_button v0ck_playtime" tabindex="-1">00:00 / 00:00</button>
  <span style="flex: 30"></span>
  
  <div class="v0ck_settings_container">
      <button class="v0ck_player_button v0ck_settings_btn" title="Settings" tabindex="-1">
        <svg viewBox="0 0 24 24" style="width: 20px; height: 20px;">
            <path d="M19.14,12.94c0.04-0.3,0.06-0.61,0.06-0.94c0-0.32-0.02-0.64-0.06-0.94l2.03-1.58c0.18-0.14,0.23-0.41,0.12-0.61 l-1.92-3.32c-0.12-0.22-0.37-0.29-0.59-0.22l-2.39,0.96c-0.5-0.38-1.03-0.7-1.62-0.94L14.4,2.81c-0.04-0.24-0.24-0.41-0.48-0.41 h-3.84c-0.24,0-0.43,0.17-0.47,0.41L9.25,5.35C8.66,5.59,8.12,5.92,7.63,6.29L5.24,5.33c-0.22-0.08-0.47,0-0.59,0.22L2.74,8.87 C2.62,9.08,2.66,9.34,2.86,9.48l2.03,1.58C4.84,11.36,4.8,11.69,4.8,12s0.02,0.64,0.06,0.94l-2.03,1.58 c-0.18,0.14-0.23,0.41-0.12,0.61l1.92,3.32c0.12,0.22,0.37,0.29,0.59,0.22l2.39-0.96c0.5,0.38,1.03,0.7,1.62,0.94l0.36,2.54 c0.05,0.24,0.24,0.41,0.48,0.41h3.84c0.24,0,0.44-0.17,0.47-0.41l0.36-2.54c0.59-0.24,1.13-0.56,1.62-0.94l2.39,0.96 c0.22,0.08,0.47,0,0.59-0.22l1.92-3.32c0.12-0.22,0.07-0.47-0.12-0.61L19.14,12.94z M12,15.6c-1.98,0-3.6-1.62-3.6-3.6 s1.62-3.6,3.6-3.6s3.6,1.62,3.6,3.6S13.98,15.6,12,15.6z"/>
        </svg>
      </button>
      <div class="v0ck_settings_menu v0ck_hidden">
          <div class="v0ck_menu_item v0ck_bg_row">
              <span class="v0ck_switch_label">Background</span>
              <div id="togglebg" class="v0ck_cool_switch" title="Toggle Background"></div>
          </div>
          <div class="v0ck_menu_item v0ck_bg_row">
              <span class="v0ck_switch_label">Autonext</span>
              <div id="toggleautoplay" class="v0ck_cool_switch" title="Toggle Autonext"></div>
          </div>
          <div class="v0ck_menu_item v0ck_bg_row">
              <span class="v0ck_switch_label">Danmaku</span>
              <div id="toggledanmaku" class="v0ck_cool_switch" title="Toggle Danmaku comments"></div>
          </div>
          <button id="v0ck_download" class="v0ck_menu_item" title="Download File" tabindex="-1">Download${size ? ` (${size})` : ''}</button>
      </div>
  </div>

  <button class="v0ck_player_button v0ck_toggle v0ck_fs_btn" title="Full Screen" tabindex="-1">
    <svg style="width: 20px; height: 20px;"><use id="v0ck_svg_fullscreen" href="${svg}#fullscreen"></use></svg>
  </button>
</div>
<div class="v0ck_loader v0ck_hidden"><div></div></div>
<div class="v0ck_speed_indicator v0ck_hidden">
  <svg viewBox="0 0 24 24" style="width: 16px; height: 16px; fill: currentColor; display: inline-block; vertical-align: middle; margin-right: 6px;">
    <path d="M4 18l8.5-6L4 6v12zm9-12v12l8.5-6L13 6z"/>
  </svg>
  <span>2X Speed</span>
</div>
<div class="v0ck_overlay">
  <svg style="width: 60px; height: 60px;">
    <use href="${svg}#play"></use>
  </svg>
</div>
<div class="v0ck_hud v0ck_hidden">
  <svg>
    <use class="v0ck_hud_icon v0ck_hud_volume_full" href="${svg}#volume_full"></use>
    <use class="v0ck_hud_icon v0ck_hud_volume_mid v0ck_hidden" href="${svg}#volume_mid"></use>
    <use class="v0ck_hud_icon v0ck_hud_volume_mute v0ck_hidden" href="${svg}#volume_mute"></use>
  </svg>
  <div class="v0ck_hud_bar_container">
    <div class="v0ck_hud_bar"></div>
  </div>
</div>`;

const isMobile = /Mobi/i.test(navigator.userAgent) || ('ontouchstart' in window) || (navigator.maxTouchPoints > 0);

let mouseX = -1, mouseY = -1;
const updateHoverStates = () => {
  // Block interaction if content warning is visible or on mobile
  const cwModal = document.getElementById('content-warning-modal');
  if ((cwModal && cwModal.style.display !== 'none') || isMobile) return;

  document.querySelectorAll('.v0ck').forEach(p => {
    const rect = p.getBoundingClientRect();
    const isOver = mouseX >= rect.left && mouseX <= rect.right &&
      mouseY >= rect.top && mouseY <= rect.bottom;
    p.classList.toggle("v0ck_hover", isOver);
    const gal = p.closest('.album-gallery-container');
    if (gal) gal.classList.toggle("v0ck_hover", isOver);
  });
};

document.addEventListener('mousemove', e => {
  mouseX = e.clientX;
  mouseY = e.clientY;
  updateHoverStates();
}, { passive: true });

window.addEventListener('scroll', updateHoverStates, { passive: true });
window.addEventListener('wheel', updateHoverStates, { passive: true });

class v0ck {
  constructor(elem) {
    const tagName = elem.tagName.toLowerCase();
    if (["video", "audio"].includes(tagName)) {
      const parent = elem.parentElement;
      if (parent.querySelector('.v0ck_player_controls')) {
        window.f0ckDebug("[v0ck] Player controls already exist, skipping injection and init");
        return elem; // Return the video element as the constructor result
      } else {
        parent.classList.add("v0ck", "paused");
        elem.classList.add("v0ck_video", "viewer");

        // Check if mouse is already inside the element synchronously to avoid transition flicker
        const rect = parent.getBoundingClientRect();
        if (mouseX >= rect.left && mouseX <= rect.right &&
          mouseY >= rect.top && mouseY <= rect.bottom) {
          parent.classList.add("v0ck_hover", "v0ck_no_transition");
          const gal = parent.closest('.album-gallery-container');
          if (gal) gal.classList.add("v0ck_hover");
          // Remove no-transition after a frame
          setTimeout(() => parent.classList.remove("v0ck_no_transition"), 50);
        }



        if (!document.querySelector('link[href^="/s/css/v0ck.css"]')) {
          document.head.insertAdjacentHTML("beforeend", `<link rel="stylesheet" href="/s/css/v0ck.css">`); // inject css
        }

        // Use absolute path for reliable asset loading
        const size = elem.getAttribute('data-size');
        elem.insertAdjacentHTML("afterend", tpl_player(`/s/img/v0ck.svg`, size));
        window.f0ckDebug("[v0ck] Player initialized for", tagName);
      }

      if (tagName === "audio") {
        const poster = elem.getAttribute('poster');
        const player = elem.closest('.v0ck') || elem.parentElement;
        const isFallback = !poster || poster === '/s/img/200.gif' || poster.includes('audio.webp') || poster.includes('music.webp');
        if (player) {
          let ph = player.querySelector(':scope > .sidebar-media-placeholder.audio');
          if (!ph) {
            ph = document.createElement('div');
            ph.className = 'sidebar-media-placeholder audio';
            ph.innerHTML = '<div class="audio-cover-circle"><i class="fa-solid fa-music"></i></div>';
            player.prepend(ph);
          }
          let coverCircle = ph.querySelector('.audio-cover-circle');
          if (!coverCircle) {
            coverCircle = document.createElement('div');
            coverCircle.className = 'audio-cover-circle';
            coverCircle.innerHTML = '<i class="fa-solid fa-music"></i>';
            ph.insertBefore(coverCircle, ph.firstChild);
          } else {
            if (!coverCircle.querySelector('i')) {
              const existingI = ph.querySelector(':scope > i');
              if (existingI) {
                coverCircle.appendChild(existingI);
              } else {
                coverCircle.insertAdjacentHTML('beforeend', '<i class="fa-solid fa-music"></i>');
              }
            }
          }
          player.style.backgroundImage = 'none';
          player.style.backgroundColor = 'transparent';
          if (!isFallback) {
            coverCircle._origBgImage = `url('${poster}')`;
            if (typeof window.updateCoverArtSolidMode === 'function') {
              window.updateCoverArtSolidMode();
            } else {
              const tuning = window.audioVisualizerTuning;
              const showInEye = tuning ? (tuning.showCoverInEye !== undefined ? Number(tuning.showCoverInEye) === 1 : (tuning.showCoverArt !== undefined ? Number(tuning.showCoverArt) === 1 : Number(tuning.solidCover) === 0)) : false;
              if (showInEye) {
                coverCircle.style.backgroundImage = `url('${poster}')`;
                ph.classList.add('has-cover');
              } else {
                coverCircle.style.backgroundImage = 'none';
                ph.classList.remove('has-cover');
              }
            }
          } else {
            coverCircle._origBgImage = null;
            coverCircle.style.backgroundImage = 'none';
            ph.classList.remove('has-cover');
            if (typeof window.updateCoverArtSolidMode === 'function') {
              window.updateCoverArtSolidMode();
            }
          }
        }
      }
    }
    else
      return console.error("nope");
    const inst = this.init(elem);
    if (elem && elem.tagName === 'AUDIO' && window.initVisualizer) {
      window.initVisualizer(elem);
    }
    return inst;
  }

  init(elem) {
    const player = elem.closest('.v0ck') || document.querySelector('.v0ck');
    const video = elem;
    video.removeAttribute('controls');
    video.removeAttribute('autoplay');
    video.addEventListener('contextmenu', e => {
      if (isMobile) e.preventDefault(); // Block native download/options menu on mobile only
    });
    const progress = player.querySelector('.v0ck_progress');
    const progressBar = player.querySelector('.v0ck_progress_filled');
    const bufferBar = player.querySelector('.v0ck_progress_buffered');
    const seekMarker = player.querySelector('.v0ck_seek_marker');
    const loader = player.querySelector('.v0ck_loader');
    const toggle = player.querySelector('.v0ck_toggle');
    const skipButtons = player.querySelectorAll('.v0ck [data-skip]');
    const ranges = player.querySelectorAll('.v0ck input[type="range"]');
    const volumeSlider = player.querySelector('.v0ck input[type="range"][name="volume"]');
    const fullscreen = player.querySelector('.v0ck_fs_btn');
    const playtime = player.querySelector('.v0ck_playtime');
    const overlay = player.querySelector('.v0ck_overlay');
    const volumeButton = player.querySelector('.v0ck_volume');
    const volumeSymbols = volumeButton.querySelectorAll('use');

    const defaultVolume = 0.5;
    let mousedown = false;
    let _volume;
    
    // Hold to speedup (2x) states
    let speedUpTimeout;
    let isSpeedingUp = false;
    let restorePlaybackRate = 1;
    let ignoreNextClick = false;
    let wasPausedWhenStarted = false;
    // Mobile tap-to-show-controls: true when this touch revealed the controls bar
    let controlsJustShown = false;

    const setHover = (active) => {
      if (active) {
        player.classList.add('v0ck_hover');
        const gal = player.closest('.album-gallery-container');
        if (gal) gal.classList.add('v0ck_hover');
      } else {
        player.classList.remove('v0ck_hover');
        const gal = player.closest('.album-gallery-container');
        if (gal) {
          gal.classList.remove('v0ck_hover');
          gal.classList.remove('strip-peek');
          const strip = gal.querySelector('.album-thumbnails-strip');
          if (strip) strip.classList.remove('strip-peek');
        }
      }
    };
    const speedIndicator = player.querySelector('.v0ck_speed_indicator');
    
    // (mouse position is now tracked via docMouseX/docMouseY in resetControlsTimer block)

    function handleVolumeButton(vol) {
      [...volumeSymbols].forEach(s => s.classList.add('v0ck_hidden'));
      let targetId = 'v0ck_svg_volume_full';
      if (vol === 0) {
        targetId = 'v0ck_svg_volume_mute';
      } else if (vol <= 0.5) {
        targetId = 'v0ck_svg_volume_mid';
      }
      const activeSymbol = [...volumeSymbols].find(s => s.id === targetId);
      if (activeSymbol) {
        activeSymbol.classList.remove('v0ck_hidden');
      }
      localStorage.setItem("volume", vol);
    }
    function togglePlay() {
      return video[video.paused ? 'play' : 'pause']();
    }
    function updatePlayIcon() {
      const isPlaying = !video.paused;
      toggle.classList.toggle('playing', isPlaying);
      player.classList.toggle('paused', !isPlaying);
      toggle.setAttribute('title', isPlaying ? 'Pause' : 'Play');
      const playIcon = toggle.querySelector('#v0ck_svg_play');
      const pauseIcon = toggle.querySelector('#v0ck_svg_pause');
      if (playIcon && pauseIcon) {
        playIcon.classList.toggle('v0ck_hidden', isPlaying);
        pauseIcon.classList.toggle('v0ck_hidden', !isPlaying);
      } else {
        [...toggle.querySelectorAll('use')].forEach(icon => {
          const isPlaySvg = icon.id === 'v0ck_svg_play' || icon.getAttribute('href')?.includes('play');
          icon.classList.toggle('v0ck_hidden', isPlaySvg ? isPlaying : !isPlaying);
        });
      }
    }
    function toggleMute(e) {
      if (video.volume === 0)
        video.volume = volumeSlider.value = _volume === 0 ? defaultVolume : _volume;
      else {
        _volume = video.volume;
        video.volume = volumeSlider.value = 0;
      }
      handleVolumeButton(video.volume);
    }
    function skip() {
      video.currentTime += +this.dataset.skip;
    }
    function handleRangeUpdate() {
      video[this.name] = this.value;
      _volume = video.volume;
      handleVolumeButton(video.volume);
    }
    function formatTime(seconds) {
      const minutes = (~~(seconds / 60)).toString().padStart(2, "0");
      seconds = (~~(seconds % 60)).toString().padStart(2, "0");
      return minutes + ":" + seconds;
    }
    function handleProgress() {
      const percent = (video.currentTime / video.duration) * 100;
      progressBar.style.flexBasis = percent + '%';
      playtime.innerText = `${formatTime(video.currentTime)} / ${formatTime(video.duration)}`;
      
      if (video.buffered.length > 0) {
        const bufferedEnd = video.buffered.end(video.buffered.length - 1);
        const duration = video.duration;
        if (duration > 0) {
          bufferBar.style.width = (bufferedEnd / duration) * 100 + '%';
        }
      }
    }
    function scrub(e) {
      let x;
      if (e.type.startsWith('touch')) {
         const rect = progress.getBoundingClientRect();
         x = e.touches[0].clientX - rect.left;
         if (e.type === 'touchmove' && e.cancelable) e.preventDefault(); 
      } else {
         x = e.offsetX;
      }
      
      const width = progress.offsetWidth;
      // Clamp x between 0 and width
      x = Math.max(0, Math.min(x, width));

      const scrubTime = (x / width) * video.duration;
      if (!Number.isFinite(scrubTime)) return;

      video.currentTime = scrubTime;
      
      // Visual seek marker
      seekMarker.style.left = `${(x / width) * 100}%`;
      seekMarker.classList.remove('active');
      void seekMarker.offsetWidth; // trigger reflow
      seekMarker.classList.add('active');
    }
    function enterFullScreen() {
      if (document.fullscreenElement) return;
      const target = player;
      if (/(iPad|iPhone|iPod)/gi.test(navigator.platform))
        video.webkitEnterFullscreen();
      else
        target.requestFullscreen();
    }
    function toggleFullScreen(e) {
      if (document.fullscreenElement) // exit fullscreen
        document.exitFullscreen();
      else { // request fullscreen
        enterFullScreen();
      }
    }
    function toggleFullScreenClasses() {
      const fsElem = document.fullscreenElement;
      const isThisPlayerFS = fsElem && (fsElem === player || fsElem.contains(player));
      player.classList.toggle('v0ck_fullscreen', !!isThisPlayerFS);
    }

    // Mobile: on touchstart, record whether controls were hidden so the
    // subsequent click can decide whether to show controls or toggle play.
    player.addEventListener('touchstart', () => {
      if (isMobile) {
        controlsJustShown = !player.classList.contains('v0ck_hover');
      }
    }, { passive: true, capture: true });

    player.addEventListener('click', e => {
      if (ignoreNextClick) {
        e.stopPropagation();
        e.preventDefault();
        ignoreNextClick = false;
        return;
      }
      const path = e.path || (e.composedPath && e.composedPath());
      const isControls = !!path.filter(f => f.classList?.contains('v0ck_player_controls')).length;
      if (!isControls) {
        if (isMobile && controlsJustShown) {
          // First tap: controls were just revealed by this touch — don't toggle play
          controlsJustShown = false;
          setHover(true);
          return;
        }
        controlsJustShown = false;
        togglePlay(e);
      }
    });
    toggle.addEventListener('click', togglePlay);
    overlay.addEventListener('click', e => {
      e.stopPropagation();
      setHover(true);
      togglePlay();
    });
    video.addEventListener('play', updatePlayIcon);
    
    // Robust initial overlay removal
    const removeInitial = () => player.classList.remove('v0ck_initial');
    video.addEventListener('play', removeInitial);
    video.addEventListener('playing', removeInitial);
    video.addEventListener('timeupdate', () => {
        if (video.currentTime > 0.1 && !video.paused && !video.ended) removeInitial();
    });

    video.addEventListener('pause', updatePlayIcon);
    video.addEventListener('timeupdate', handleProgress);
    video.addEventListener('progress', handleProgress);
    video.addEventListener('ended', () => {
        if (localStorage.getItem('autoplay') === 'true') {
            const nextBtn = document.getElementById('next');
            if (nextBtn && nextBtn.href && !nextBtn.href.endsWith('#')) {
                nextBtn.click();
            }
        }
    });

    // Loader events with debounce to prevent flicker on fast starts/seeking
    let loaderTimeout = null;
    const showLoader = () => {
      if (loaderTimeout) clearTimeout(loaderTimeout);
      loaderTimeout = setTimeout(() => {
        loader.classList.remove('v0ck_hidden');
      }, 250);
    };
    const hideLoader = () => {
      if (loaderTimeout) clearTimeout(loaderTimeout);
      loader.classList.add('v0ck_hidden');
    };
    video.addEventListener('waiting', showLoader);
    video.addEventListener('stalled', showLoader);
    video.addEventListener('canplay', hideLoader);
    video.addEventListener('playing', hideLoader);
    video.addEventListener('seeked', hideLoader);
    video.addEventListener('loadeddata', hideLoader);
    volumeButton.addEventListener('click', e => {
      e.stopPropagation();
      toggleMute(e);
    });

    const hud = player.querySelector('.v0ck_hud');
    const hudBar = hud.querySelector('.v0ck_hud_bar');
    const hudIcon = hud.querySelector('.v0ck_hud_icon');
    let startX, startY, startVol, isRightSide, gestureType;
    let hudTimer;

    function showHUD(vol) {
      hud.classList.remove('v0ck_hidden');
      hudBar.style.width = `${vol * 100}%`;
      
      // Update HUD icon based on volume by toggling hidden class
      const hudSymbols = hud.querySelectorAll('.v0ck_hud_icon');
      hudSymbols.forEach(s => s.classList.add('v0ck_hidden'));
      
      let targetClass = 'v0ck_hud_volume_full';
      if (vol === 0) {
        targetClass = 'v0ck_hud_volume_mute';
      } else if (vol <= 0.5) {
        targetClass = 'v0ck_hud_volume_mid';
      }
      
      const activeSymbol = [...hudSymbols].find(s => s.classList.contains(targetClass));
      if (activeSymbol) {
        activeSymbol.classList.remove('v0ck_hidden');
      }

      clearTimeout(hudTimer);
      hudTimer = setTimeout(() => hud.classList.add('v0ck_hidden'), 1000);
    }

    player.addEventListener('touchstart', e => {
      if (!isMobile) return;
      const touch = e.touches[0];
      const rect = player.getBoundingClientRect();
      const x = touch.clientX - rect.left;
      isRightSide = x > rect.width / 2;
      gestureType = 'none';
      
      if (isRightSide) {
        startX = touch.clientX;
        startY = touch.clientY;
        startVol = video.volume;
      }
    }, { passive: false });

    player.addEventListener('touchmove', e => {
      if (!isMobile || !isRightSide || gestureType === 'other') return;
      
      const touch = e.touches[0];
      const dx = Math.abs(touch.clientX - startX);
      const dy = Math.abs(touch.clientY - startY);

      // Identify gesture type if not yet locked
      if (gestureType === 'none') {
        if (dy > dx && dy > 5) {
          gestureType = 'volume';
          clearTimeout(speedUpTimeout);
          endSpeedUp();
        } else if (dx > dy && dx > 5) {
          gestureType = 'other'; // Probably seeking or horizontal swipe
          return;
        } else {
          return; // Too small movement to decide
        }
      }

      if (gestureType === 'volume') {
        clearTimeout(speedUpTimeout);
        endSpeedUp();
        
        const deltaY = startY - touch.clientY; // swipe up is positive
        const sensitivity = 200; // pixels for 0 to 1 range (reverted to original)
        let newVol = startVol + (deltaY / sensitivity);
        newVol = Math.max(0, Math.min(1, newVol));
        
        video.volume = newVol; // Set directly for smoothness
        volumeSlider.value = newVol; // Update visual slider
        _volume = newVol;
        handleVolumeButton(newVol);
        showHUD(newVol);
        
        if (e.cancelable) e.preventDefault();
      }
    }, { passive: false });

    // Desktop mouse volume gesture support (clicking and dragging vertically on the player)
    let activeMouseGesture = false;

    player.addEventListener('mousedown', e => {
      if (isMobile) return;
      if (e.button !== 0) return;
      const path = e.path || (e.composedPath && e.composedPath());
      const isControls = !!path.filter(f => f.classList?.contains('v0ck_player_controls')).length;
      if (isControls) return;

      gestureType = 'none';
      startX = e.clientX;
      startY = e.clientY;
      startVol = video.volume;
      activeMouseGesture = true;
    });

    window.addEventListener('mousemove', e => {
      if (!activeMouseGesture || gestureType === 'other') return;

      const dx = Math.abs(e.clientX - startX);
      const dy = Math.abs(e.clientY - startY);

      if (gestureType === 'none') {
        if (dy > dx && dy > 5) {
          gestureType = 'volume';
          clearTimeout(speedUpTimeout);
          endSpeedUp();
        } else if (dx > dy && dx > 5) {
          gestureType = 'other';
          return;
        } else {
          return;
        }
      }

      if (gestureType === 'volume') {
        clearTimeout(speedUpTimeout);
        endSpeedUp();
        ignoreNextClick = true;

        const deltaY = startY - e.clientY; // swipe up is positive
        const sensitivity = 200;
        let newVol = startVol + (deltaY / sensitivity);
        newVol = Math.max(0, Math.min(1, newVol));

        video.volume = newVol;
        volumeSlider.value = newVol;
        _volume = newVol;
        handleVolumeButton(newVol);
        showHUD(newVol);

        e.preventDefault();
      }
    });

    window.addEventListener('mouseup', () => {
      if (activeMouseGesture) {
        activeMouseGesture = false;
        setTimeout(() => {
          ignoreNextClick = false;
        }, 100);
      }
    });

    skipButtons.forEach(button => button.addEventListener('click', skip));
    ranges.forEach(range => range.addEventListener('change', handleRangeUpdate));
    ranges.forEach(range => range.addEventListener('input', handleRangeUpdate));
    ranges.forEach(range => range.addEventListener('mousemove', handleRangeUpdate));
    
    // Prevent touch events on the volume slider from bubbling to the player container (avoiding gesture conflicts and page scrolls)
    if (volumeSlider) {
      ['touchstart', 'touchmove', 'touchend', 'touchcancel'].forEach(evt => {
        volumeSlider.addEventListener(evt, e => {
          e.stopPropagation();
        }, { passive: false });
      });
    }

    progress.addEventListener('mousedown', scrub);
    progress.addEventListener('touchstart', scrub, { passive: false });
    progress.addEventListener('touchmove', scrub, { passive: false });
    fullscreen.addEventListener('click', toggleFullScreen);
    document.addEventListener('fullscreenchange', toggleFullScreenClasses);
    toggleFullScreenClasses(); // Check initial state (important for transitions)

    let keyHoldTimer = null;
    let isHoldingKey = false;
    let heldKey = null;
    let reverseInterval = null;

    function handleKeyDown(e) {
      if (!document.body.contains(video)) {
        cleanupKeyboardListeners();
        return;
      }

      // Block interaction if content warning is visible
      const cwModal = document.getElementById('content-warning-modal');
      if (cwModal && cwModal.style.display !== 'none') return;

      // Don't fire if user is typing in an input, textarea or editing content
      if (e.target.tagName === "INPUT" || e.target.tagName === "TEXTAREA" || e.target.isContentEditable) return;

      if (e.key === "ArrowUp" || e.key === "ArrowDown") {
        e.preventDefault();
        let newVol = video.volume;
        if (e.key === "ArrowUp") {
          newVol = Math.min(1, newVol + 0.05);
        } else {
          newVol = Math.max(0, newVol - 0.05);
        }
        video.volume = newVol;
        if (volumeSlider) {
          volumeSlider.value = newVol;
        }
        _volume = newVol;
        handleVolumeButton(newVol);
        showHUD(newVol);
      }

      if (e.key === "." || e.key === ",") {
        if (e.ctrlKey || e.metaKey || e.altKey) return;
        e.preventDefault();
        if (!Number.isFinite(video.currentTime)) return;
        const duration = Number.isFinite(video.duration) ? video.duration : video.currentTime;
        const direction = e.key === "." ? 1 : -1;

        if (heldKey && heldKey !== e.key) return;
        const isCurrentHold = heldKey === e.key;

        if (video.paused || isCurrentHold) {
          if (!e.repeat) {
            heldKey = e.key;
            isHoldingKey = false;
            if (keyHoldTimer) clearTimeout(keyHoldTimer);
            keyHoldTimer = setTimeout(() => {
              isHoldingKey = true;
              if (heldKey === ".") {
                video.play();
              } else if (heldKey === ",") {
                if (reverseInterval) clearInterval(reverseInterval);
                reverseInterval = setInterval(() => {
                  if (video.currentTime > 0) {
                    video.currentTime = Math.max(0, video.currentTime - 0.04);
                  } else {
                    clearInterval(reverseInterval);
                    reverseInterval = null;
                  }
                }, 40);
              }
            }, 200);
          } else {
            if (keyHoldTimer && !isHoldingKey) {
              clearTimeout(keyHoldTimer);
              keyHoldTimer = null;
              isHoldingKey = true;
              if (heldKey === ".") {
                video.play();
              } else if (heldKey === ",") {
                if (reverseInterval) clearInterval(reverseInterval);
                reverseInterval = setInterval(() => {
                  if (video.currentTime > 0) {
                    video.currentTime = Math.max(0, video.currentTime - 0.04);
                  } else {
                    clearInterval(reverseInterval);
                    reverseInterval = null;
                  }
                }, 40);
              }
            }
          }
        } else {
          video.currentTime = Math.max(0, Math.min(duration, video.currentTime + direction * 5));
        }
        showControlsAndReset();
      }
    }

    function handleKeyUp(e) {
      if (!document.body.contains(video)) {
        cleanupKeyboardListeners();
        return;
      }
      if (e.key === "." || e.key === ",") {
        if (heldKey === e.key) {
          if (keyHoldTimer) {
            clearTimeout(keyHoldTimer);
            keyHoldTimer = null;
          }
          if (isHoldingKey) {
            if (heldKey === ".") {
              video.pause();
            } else if (heldKey === ",") {
              if (reverseInterval) {
                clearInterval(reverseInterval);
                reverseInterval = null;
              }
            }
          } else {
            if (Number.isFinite(video.currentTime)) {
              const duration = Number.isFinite(video.duration) ? video.duration : video.currentTime;
              const direction = heldKey === "." ? 1 : -1;
              const frameTime = 1 / 30;
              video.currentTime = Math.max(0, Math.min(duration, video.currentTime + direction * frameTime));
            }
          }
          heldKey = null;
          isHoldingKey = false;
          showControlsAndReset();
        }
      }
    }

    function handleBlur() {
      if (!document.body.contains(video)) {
        cleanupKeyboardListeners();
        return;
      }
      if (keyHoldTimer) clearTimeout(keyHoldTimer);
      if (reverseInterval) clearInterval(reverseInterval);
      keyHoldTimer = null;
      reverseInterval = null;
      if (isHoldingKey) {
        if (heldKey === ".") {
          video.pause();
        }
      }
      heldKey = null;
      isHoldingKey = false;
    }

    function cleanupKeyboardListeners() {
      document.removeEventListener('keydown', handleKeyDown);
      document.removeEventListener('keyup', handleKeyUp);
      window.removeEventListener('blur', handleBlur);
      if (keyHoldTimer) clearTimeout(keyHoldTimer);
      if (reverseInterval) clearInterval(reverseInterval);
    }

    document.addEventListener('keydown', handleKeyDown);
    document.addEventListener('keyup', handleKeyUp);
    window.addEventListener('blur', handleBlur);

    video.volume = _volume = volumeSlider.value = +(localStorage.getItem('volume') ?? defaultVolume);
    handleVolumeButton(video.volume);

    const mediaObj = player.closest('.media-object');
    let isBlurredDetail = false;
    if (mediaObj && localStorage.getItem('blurDetail') !== 'false') {
      const mode = mediaObj.getAttribute('data-mode');
      const blurNsfw = localStorage.getItem('blurNsfw') === 'true';
      const blurNsfl = localStorage.getItem('blurNsfl') === 'true';
      const blurSfw = localStorage.getItem('blurSfw') === 'true';
      const blurUntagged = localStorage.getItem('blurUntagged') === 'true';
      
      let shouldBlurThis = false;
      if (mode === 'nsfw') shouldBlurThis = blurNsfw;
      else if (mode === 'nsfl') shouldBlurThis = blurNsfl;
      else if (mode === 'sfw') shouldBlurThis = blurSfw;
      else if (mode === 'untagged') shouldBlurThis = blurUntagged;
      
      if (shouldBlurThis && !mediaObj.classList.contains('revealed')) {
        isBlurredDetail = true;
      }
    }

    // Upgrade preload from 'metadata' → 'auto' for video elements so the browser
    // starts buffering data immediately during player setup. With just 'metadata',
    // play() triggers a brand-new range request and the user sees the loader spinner
    // until enough data arrives. 'auto' closes that gap for both autoplay and manual play.
    if (video.tagName === 'VIDEO' && video.preload !== 'auto') {
      video.preload = 'auto';
    }

    // Attempt autoplay and show overlay if blocked
    const shouldAutoplay = !isBlurredDetail && window.f0ckSession?.disable_autoplay !== true;
    if (shouldAutoplay) {
      if (!video.paused) {
        player.classList.remove('v0ck_initial');
      } else {
        const playPromise = video.play();
        if (playPromise !== undefined) {
          playPromise.then(() => {
            player.classList.remove('v0ck_initial');
          }).catch((err) => {
            if (err && err.name === 'AbortError') {
              const onCanPlay = () => {
                video.removeEventListener('canplay', onCanPlay);
                if (video.paused) {
                  video.play().then(() => {
                    player.classList.remove('v0ck_initial');
                  }).catch(() => {
                    player.classList.add('v0ck_initial');
                  });
                }
              };
              video.addEventListener('canplay', onCanPlay, { once: true });
            } else {
              player.classList.add('v0ck_initial');
            }
          });
        } else if (video.paused) {
          player.classList.add('v0ck_initial');
        }
      }
    } else {
      player.classList.add('v0ck_initial');
    }

    // Settings Menu Logic
    const settingsBtn = player.querySelector('.v0ck_settings_btn');
    const settingsMenu = player.querySelector('.v0ck_settings_menu');
    const toggleBgSwitch = player.querySelector('#togglebg');
    const downloadBtn = player.querySelector('#v0ck_download');

    if (downloadBtn) {
        downloadBtn.addEventListener('click', (e) => {
            e.stopPropagation();
            const a = document.createElement('a');
            a.href = video.src;
            a.download = video.src.split('/').pop();
            document.body.appendChild(a);
            a.click();
            document.body.removeChild(a);
        });
    }

    // Initialize switch state
    const bgEnabled = (window.f0ckSession && window.f0ckSession.show_background !== undefined)
        ? !!window.f0ckSession.show_background
        : (localStorage.getItem('background') !== 'false');
    
    if (toggleBgSwitch && bgEnabled) {
        toggleBgSwitch.classList.add('active');
    }
    
    const toggleAutoplaySwitch = player.querySelector('#toggleautoplay');
    if (toggleAutoplaySwitch && localStorage.getItem('autoplay') === 'true') {
        toggleAutoplaySwitch.classList.add('active');
        video.loop = false;
    }
    
    if(settingsBtn && settingsMenu) {
        settingsBtn.addEventListener('click', (e) => {
            e.stopPropagation();
            settingsMenu.classList.toggle('v0ck_hidden');
            if (settingsMenu.classList.contains('v0ck_hidden')) {
                document.dispatchEvent(new CustomEvent('v0ck_settings_closed'));
            }
        });

        // Close menu/panel when clicking outside
        document.addEventListener('click', (e) => {
             if (!e.isTrusted) return;
             const isFlashYankUI = e.target.closest('#flash-yank-ui');
             const isInsidePlayer = player.contains(e.target);
             
             if(!settingsMenu.contains(e.target) && !settingsBtn.contains(e.target) && !isFlashYankUI) {
                 if (!settingsMenu.classList.contains('v0ck_hidden')) {
                     settingsMenu.classList.add('v0ck_hidden');
                     document.dispatchEvent(new CustomEvent('v0ck_settings_closed'));
                 }
             }

             if (isMobile && !isInsidePlayer && !isFlashYankUI) {
                 setHover(false);
             }
        });
        
        // Prevent menu click from pausing video (only if clicked on non-button area)
        settingsMenu.addEventListener('click', (e) => {
            if (!e.target.closest('button') && !e.target.closest('.v0ck_bg_row')) {
                 e.stopPropagation();
            }
        });

        // Visual toggle for background switch
        if (toggleBgSwitch) {
            const bgRow = toggleBgSwitch.closest('.v0ck_bg_row');
            const handleBgToggle = (e) => {
                e.stopPropagation();
                if (window.toggleBackground) {
                    window.toggleBackground();
                } else {
                    toggleBgSwitch.classList.toggle('active');
                }
            };
            if (bgRow) bgRow.addEventListener('click', handleBgToggle);
            else toggleBgSwitch.addEventListener('click', handleBgToggle);
        }

        if (toggleAutoplaySwitch) {
            const autoplayRow = toggleAutoplaySwitch.closest('.v0ck_bg_row');
            const handleAutoplayToggle = (e) => {
                e.stopPropagation();
                if (window.toggleAutoplay) {
                    window.toggleAutoplay();
                } else {
                    toggleAutoplaySwitch.classList.toggle('active');
                }
                video.loop = !toggleAutoplaySwitch.classList.contains('active');
            };
            if (autoplayRow) autoplayRow.addEventListener('click', handleAutoplayToggle);
            else toggleAutoplaySwitch.addEventListener('click', handleAutoplayToggle);
        }

        // Danmaku toggle
        const toggleDanmakuSwitch = player.querySelector('#toggledanmaku');
        if (toggleDanmakuSwitch) {
            // Sync initial state from localStorage vs site-wide config default
            const configDefault = (window.f0ckSession && window.f0ckSession.enable_danmaku !== undefined)
                ? !!window.f0ckSession.enable_danmaku
                : true;

            const savedPref = localStorage.getItem('danmaku');
            const isEnabled = (savedPref !== null) ? (savedPref !== 'false') : configDefault;

            if (isEnabled) {
                toggleDanmakuSwitch.classList.add('active');
            }
            const danmakuRow = toggleDanmakuSwitch.closest('.v0ck_bg_row');
            const handleDanmakuToggle = (e) => {
                e.stopPropagation();
                if (window.danmakuInstance) {
                    window.danmakuInstance.toggle();
                    const on = window.danmakuInstance.isEnabled();
                    toggleDanmakuSwitch.classList.toggle('active', on);
                    if (window.flashMessage) window.flashMessage(on ? 'Danmaku ON' : 'Danmaku OFF', 1800);
                } else {
                    toggleDanmakuSwitch.classList.toggle('active');
                    const newVal = toggleDanmakuSwitch.classList.contains('active');
                    localStorage.setItem('danmaku', newVal ? 'true' : 'false');
                    if (window.flashMessage) window.flashMessage(newVal ? 'Danmaku ON' : 'Danmaku OFF', 1800);
                }
            };
            if (danmakuRow) danmakuRow.addEventListener('click', handleDanmakuToggle);
            else toggleDanmakuSwitch.addEventListener('click', handleDanmakuToggle);
        }
    }

    // Controls auto-hide logic (auto hide controls after 2.5 seconds of inactivity)
    let controlsTimer;
    // True while the cursor is physically inside .v0ck_player_controls
    let mouseIsOverControls = false;

    // Track real mouse position at document level — completely independent of
    // any element animation or synthetic events.
    let docMouseX = -1;
    let docMouseY = -1;
    const onDocMouseMove = (e) => {
      docMouseX = e.clientX;
      docMouseY = e.clientY;
    };
    document.addEventListener('mousemove', onDocMouseMove, { passive: true });

    function resetControlsTimer() {
      clearTimeout(controlsTimer);
      // Never schedule auto-hide while the user is mousing over the controls bar
      if (mouseIsOverControls) return;
      const isFullscreen = player.classList.contains('v0ck_fullscreen');
      if (!video.paused || isFullscreen) {
        controlsTimer = setTimeout(() => {
          setHover(false);
          if (settingsMenu && !settingsMenu.classList.contains('v0ck_hidden')) {
            settingsMenu.classList.add('v0ck_hidden');
            document.dispatchEvent(new CustomEvent('v0ck_settings_closed'));
          }
        }, 2500);
      }
    }

    function showControlsAndReset(e) {
      // Ignore synthetic pointer events fired by the browser during CSS animations
      // (element shifts under stationary cursor). We compare against docMouseX/Y
      // which is only ever updated by genuine user mouse movement.
      if (e && e.clientX !== undefined && e.clientY !== undefined) {
        if (e.clientX === docMouseX && e.clientY === docMouseY &&
            player.classList.contains('v0ck_hover')) {
          // Coordinates unchanged and controls already visible — synthetic event, skip.
          return;
        }
        docMouseX = e.clientX;
        docMouseY = e.clientY;
      }
      setHover(true);
      resetControlsTimer();
    }

    // Events that should show controls and reset/extend the auto-hide timer
    const resetEvents = ['touchstart', 'touchmove', 'touchend', 'click', 'mousemove', 'mouseenter'];
    resetEvents.forEach(evt => {
      player.addEventListener(evt, showControlsAndReset, { capture: true, passive: true });
    });

    // While hovering the controls bar: freeze the auto-hide timer completely.
    const controlsBar = player.querySelector('.v0ck_player_controls');
    if (controlsBar) {
      controlsBar.addEventListener('mouseenter', () => {
        mouseIsOverControls = true;
        clearTimeout(controlsTimer); // cancel any countdown already in progress
      }, { passive: true });
      controlsBar.addEventListener('mouseleave', (e) => {
        mouseIsOverControls = false;
        // Only restart the timer if the cursor re-entered the player area
        // (not when it left the player entirely — the player mouseleave handles that)
        const r = player.getBoundingClientRect();
        const stillInPlayer = e.clientX >= r.left && e.clientX <= r.right &&
                              e.clientY >= r.top  && e.clientY <= r.bottom;
        if (stillInPlayer) resetControlsTimer();
      }, { passive: true });
    }

    // Hide when cursor leaves the player entirely.
    // NO capture:true — that would incorrectly intercept mouseleave events from
    // child elements (e.g. the progress bar animating away from the cursor).
    // Without capture, this only fires when the mouse truly leaves .v0ck itself.
    player.addEventListener('mouseleave', (e) => {
      const r = player.getBoundingClientRect();
      // Grace zone: the controls bar peeks ~3px below the player when hidden.
      // If the cursor is still in that strip, don't hide.
      if (e.clientX >= r.left && e.clientX <= r.right &&
          e.clientY > r.bottom && e.clientY <= r.bottom + 8) {
        return;
      }
      // If the cursor moved into the controls bar itself (or any child of it),
      // keep the controls visible — the user is interacting with them.
      const controls = player.querySelector('.v0ck_player_controls');
      if (controls && e.relatedTarget && controls.contains(e.relatedTarget)) {
        return;
      }
      docMouseX = -1;
      docMouseY = -1;
      setHover(false);
      clearTimeout(controlsTimer);
    });

    video.addEventListener('play', resetControlsTimer);
    video.addEventListener('playing', resetControlsTimer);
    video.addEventListener('pause', () => clearTimeout(controlsTimer));

    // Speedup 2x on Hold logic
    function startSpeedUp(e) {
      if (e.type === 'mousedown' && isMobile) return;
      // Only left mouse click or touch triggers speedup
      if (e.type === 'mousedown' && e.button !== 0) return;

      // Don't speed up if clicking on controls or settings panel
      const path = e.path || (e.composedPath && e.composedPath());
      const isControls = !!path.filter(f => f.classList?.contains('v0ck_player_controls')).length;
      if (isControls) return;

      clearTimeout(speedUpTimeout);
      speedUpTimeout = setTimeout(() => {
        isSpeedingUp = true;
        ignoreNextClick = true;
        wasPausedWhenStarted = video.paused;
        restorePlaybackRate = video.playbackRate;
        video.playbackRate = 2.0;

        if (wasPausedWhenStarted) {
          video.play();
        }

        if (speedIndicator) {
          speedIndicator.classList.remove('v0ck_hidden');
        }
      }, 500);
    }

    function endSpeedUp(e) {
      clearTimeout(speedUpTimeout);
      if (isSpeedingUp) {
        isSpeedingUp = false;
        video.playbackRate = restorePlaybackRate;
        if (wasPausedWhenStarted) {
          video.pause();
          wasPausedWhenStarted = false;
        }
        if (speedIndicator) {
          speedIndicator.classList.add('v0ck_hidden');
        }
        // Brief timeout before allowing normal clicking again to bypass the immediate click event
        setTimeout(() => {
          ignoreNextClick = false;
        }, 100);
      }
    }

    player.addEventListener('mousedown', startSpeedUp);
    player.addEventListener('touchstart', startSpeedUp, { passive: true });
    player.addEventListener('mouseup', endSpeedUp);
    player.addEventListener('mouseleave', endSpeedUp);
    player.addEventListener('touchend', endSpeedUp);
    player.addEventListener('touchcancel', endSpeedUp);

    this.toggleFullScreen = toggleFullScreen;
    this.enterFullScreen = enterFullScreen;

    return video;
  }
}

window.v0ck = v0ck;
})();
