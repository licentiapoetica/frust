(function () {
    var DATA = JSON.parse(document.getElementById('meme-creator-data').textContent);
    window.memeTemplate = {
        id: DATA.id,
        url: DATA.url,
        name: DATA.name,
        category: DATA.category,
        sub_category: DATA.sub_category
    };
    window.csrf_token = DATA.csrf_token;
})();
