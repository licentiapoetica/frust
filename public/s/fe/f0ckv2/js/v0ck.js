const tpl_player = svg => `<div class="v0ck_player_controls">
  <div class="v0ck_progress">
    <div class="v0ck_progress_filled"></div>
  </div>
  <button class="v0ck_player_button v0ck_tplay v0ck_toggle" title="Play">
    <svg>
      <use id="v0ck_svg_play" href="${svg}#play"></use>
      <use id="v0ck_svg_pause" class="v0ck_hidden" href="${svg}#pause"></use>
    </svg>
  </button>
  <button class="v0ck_player_button v0ck_volume">
    <svg>
      <use id="v0ck_svg_volume_full" href="${svg}#volume_full"></use>
      <use id="v0ck_svg_volume_mid" class="v0ck_hidden" href="${svg}#volume_mid"></use>
      <use id="v0ck_svg_volume_mute" class="v0ck_hidden" href="${svg}#volume_mute"></use>
    </svg>
  </button>
  <input type="range" name="volume" min="0" max="1" step="0.01" value="1" />
  <button class="v0ck_player_button v0ck_playtime">00:00 / 00:00</button>
  <span class="v0ck_spacer"></span>
  <button id="togglebg">&#128161;</button>
  <button data-skip="-10" class="v0ck_player_button">
    <svg><use id="v0ck_svg_backward" href="${svg}#backward"></use></svg>
  </button>
  <button data-skip="10" class="v0ck_player_button">
    <svg><use id="v0ck_svg_forward" href="${svg}#forward"></use></svg>
  </button>
  <button class="v0ck_player_button v0ck_toggle v0ck_fullscreen" title="Full Screen">
    <svg><use id="v0ck_svg_fullscreen" href="${svg}#fullscreen"></use></svg>
  </button>
</div>
<div class="v0ck_overlay v0ck_hidden">
  <svg><use id="v0ck_svg_ol_play" href="${svg}#ol_play"></use></svg>
</div>`;

class v0ck {
  constructor(elem) {
    const tagName = elem.tagName.toLowerCase();
    if(["video", "audio"].includes(tagName)) {
      const parent = elem.parentElement;
      parent.classList.add("v0ck", "paused");
      elem.classList.add("v0ck_video", "viewer");
      elem.insertAdjacentHTML("afterend", tpl_player("/s/fe/f0ckv2/img/v0ck.svg"));

      if(tagName === "audio" && elem.hasAttribute('poster')) { // set cover
        // the cover is set by the template (custom property on the player, see f0ckv2.css)
        parent.classList.add('v0ck_has_cover');
      }
    }
    else
      return console.error("nope");
    return this.init(elem);
  }

  init(elem) {
    const player = document.querySelector('.v0ck');
    const video = elem;
    video.removeAttribute('controls');
    video.removeAttribute('autoplay');
    const progress = player.querySelector('.v0ck_progress');
    const progressBar = player.querySelector('.v0ck_progress_filled');
    const toggle = player.querySelector('.v0ck_toggle');
    const skipButtons = player.querySelectorAll('.v0ck [data-skip]');
    const ranges = player.querySelectorAll('.v0ck input[type="range"]');
    const volumeSlider = player.querySelector('.v0ck input[type="range"][name="volume"]');
    const fullscreen = player.querySelector('.v0ck_fullscreen');
    const playtime = player.querySelector('.v0ck_playtime');
    const overlay = player.querySelector('.v0ck_overlay');
    const volumeButton = player.querySelector('.v0ck_volume');
    const volumeSymbols = volumeButton.querySelectorAll('.v0ck use');

    const defaultVolume = 0.5;
    let mousedown = false;
    let _volume;

    function handleVolumeButton(vol) {
      [...volumeSymbols].forEach(s => !s.classList.contains('v0ck_hidden') ? s.classList.add('v0ck_hidden') : null);
      switch(true) {
        case(vol === 0):             [...volumeSymbols].filter(s => s.id === "v0ck_svg_volume_mute")[0].classList.toggle('v0ck_hidden'); break;
        case(vol <= 0.5 && vol > 0): [...volumeSymbols].filter(s => s.id === "v0ck_svg_volume_mid")[0].classList.toggle('v0ck_hidden');  break;
        case(vol > 0.5):             [...volumeSymbols].filter(s => s.id === "v0ck_svg_volume_full")[0].classList.toggle('v0ck_hidden'); break;
      }
      localStorage.setItem("volume", vol);
    }
    function togglePlay() {
      const status = video[video.paused ? 'play' : 'pause']();
      if(status !== undefined) { // todo: merge with updatePlayIcon
        status
          .then(_ => {
            overlay.classList[video.paused ? 'remove' : 'add']('v0ck_hidden');
          })
          .catch(err => {
            overlay.classList.toggle('v0ck_hidden');
          });
      }
      else
        overlay.classList[video.paused ? 'remove' : 'add']('v0ck_hidden');
    }
    function updatePlayIcon() {
      toggle.classList.toggle('playing');
      player.classList.toggle('paused');
      toggle.setAttribute('title', toggle.classList.contains('playing') ? 'Pause' : 'Play');
      [...toggle.querySelectorAll('use')].forEach(icon => icon.classList.toggle('v0ck_hidden'));
    }
    function toggleMute(e) {
      if(video.volume === 0)
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
      progressBar.style.setProperty('--v0ck-progress', percent + '%');
      playtime.innerText = `${formatTime(video.currentTime)} / ${formatTime(video.duration)}`;
    }
    function scrub(e) {
      video.currentTime = (e.offsetX / progress.offsetWidth) * video.duration;
    }
    function toggleFullScreen(e) {
      if(document.fullscreenElement) // exit fullscreen
        document.exitFullscreen();
      else { // request fullscreen
        if(/(iPad|iPhone|iPod)/gi.test(navigator.platform))
          video.webkitEnterFullscreen();
        else
          player.requestFullscreen();
      }
    }
    function toggleFullScreenClasses() {
      player.classList.toggle('fullscreen');
    }

    player.addEventListener('click', e => {
      const path = e.path || (e.composedPath && e.composedPath());
      if(path.filter(f => f.classList?.contains('v0ck_player_controls')).length)
        return;

      //navigator.userAgentData = { mobile: true };
      if(navigator.userAgentData?.mobile) { // mobile

        return;
      }

      togglePlay(e);
    });
    toggle.addEventListener('click', togglePlay);
    video.addEventListener('play', updatePlayIcon);
    video.addEventListener('pause', updatePlayIcon);
    video.addEventListener('timeupdate', handleProgress);
    volumeButton.addEventListener('click', toggleMute);
    skipButtons.forEach(button => button.addEventListener('click', skip));
    ranges.forEach(range => range.addEventListener('change', handleRangeUpdate));
    ranges.forEach(range => range.addEventListener('mousemove', handleRangeUpdate));
    progress.addEventListener('mousedown', scrub);
    fullscreen.addEventListener('click', toggleFullScreen);
    document.addEventListener('fullscreenchange', toggleFullScreenClasses);

    video.volume = _volume = volumeSlider.value = +(localStorage.getItem('volume') ?? defaultVolume);
    handleVolumeButton(video.volume);
    if(video.dataset.autoplay !== '0') // f0ckm setting "disable autoplay"
      togglePlay();
    else
      overlay.classList.remove('v0ck_hidden');

    return video;
  }
}
