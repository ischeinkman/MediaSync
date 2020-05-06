mediasync = {
    myGetPlayers: function () {
        try {
            if (!netflix || netflix === undefined) {
                return [];
            }
            if (!!!netflix.appContext || !!!netflix.appContext || !!!netflix.appContext.getPlayerApp) {
                return [];
            }
        } catch (e) {
            return [];
        }
        let player_app = netflix.appContext.getPlayerApp();
        if (!player_app) {
            return [];
        }
        rt = player_app.getState().videoPlayer;
        cadplayers = rt.cadmiumPlayerRepository.toList().map(function (k) { return k[1]; });
        htplayers = rt.htmlPlayerRepository.toList().map(function (k) { return k[1]; });
        return cadplayers.concat(htplayers);
    },
    get_pos: function () {
        var players = mediasync.myGetPlayers();
        if (players.length > 1) {
            mediasync.log('WARN', { msg: 'Multiple players found', payload: players });
        }
        else if (players.length == 0) {
            mediasync.log('WARN', { msg: 'No players found!' });
            return 0;
        }
        var pos = players[0].getCurrentTime();
        var idx = 1;
        for (idx = 1; idx < players.length; idx += 1) {
            pos = players[idx].getCurrentTime();
        }
        return pos;
    },
    get_state: function () {
        var players = mediasync.myGetPlayers();
        if (players.length > 1) {
            mediasync.log('WARN', { msg: 'Multiple players found', payload: players });
        }
        else if (players.length == 0) {
            mediasync.log('WARN', { msg: 'No players found!' });
            return true;
        }
        var stat = players[0].getPaused();
        var idx = 1;
        for (idx = 1; idx < players.length; idx += 1) {
            stat = players[idx].getPaused();
        }
        return stat;
    },
    pause: function () {
        var players = mediasync.myGetPlayers();
        if (players.length > 1) {
            mediasync.log('WARN', { msg: 'Multiple players found', payload: players });
        }
        else if (players.length == 0) {
            mediasync.log('WARN', { msg: 'No players found!' });
        }
        for (idx = 0; idx < players.length; idx += 1) {
            stat = players[idx].pause();
        }

    },
    play: function () {
        var players = mediasync.myGetPlayers();
        if (players.length > 1) {
            mediasync.log('WARN', { msg: 'Multiple players found', payload: players });
        }
        else if (players.length == 0) {
            mediasync.log('WARN', { msg: 'No players found!' });
        }
        for (idx = 0; idx < players.length; idx += 1) {
            stat = players[idx].play();
        }
    },
    seek: function (ms) {
        var players = mediasync.myGetPlayers();
        if (players.length > 1) {
            mediasync.log('WARN', { msg: 'Multiple players found', payload: players });
        }
        else if (players.length == 0) {
            mediasync.log('WARN', { msg: 'No players found!' });
        }
        for (idx = 0; idx < players.length; idx += 1) {
            stat = players[idx].seek(ms);
        }

    },
    log: function () {
        console.log(arguments);
    }
};