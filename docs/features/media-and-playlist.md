# Media and Playlist

Playing video and audio smoothly comes first in Axion. Tabs that play media are never frozen or
unloaded, and video frames are never dropped to make the interface prettier.

![The Playlist](../../assets/screenshots/playlist-dark.webp)

## Axion's video player

On a web page, every site keeps its own video controls. When a video goes **fullscreen**, Axion's
own glass controls take over:

- play and pause, jump back and forward (10 seconds), a progress bar you can drag
- volume and mute
- playback speed
- captions
- Picture in Picture
- an **Add to Playlist** button
- the controls and the mouse pointer hide after a short time without movement

Keyboard control works only while a video is fullscreen, so normal keys still reach pages the rest
of the time: **Space** or **K** to play and pause, **Left/J** and **Right/L** to seek,
**Up/Down** for volume, **M** to mute, **C** for captions, **Shift+,** and **Shift+.** for slower
and faster, **F** or **Esc** to leave. See [Keyboard shortcuts](../keyboard-shortcuts.md) to change
them.

Settings (**Settings > Media**):

- **Axion's player in fullscreen** - turn it off to keep every site's own fullscreen player.
- **Hide controls while playing after** - how long until the controls hide (1 to 10 seconds).
- **Sites that keep their own player** - sites where the site's own player stays even in fullscreen.
  YouTube, Twitch, Vimeo, Dailymotion, Netflix, Disney+, Prime Video, Crunchyroll, Spotify and
  SoundCloud are on the list to begin with. Add or remove sites there. It applies to pages loaded
  after the change.

If a video does not play at all, see [Troubleshooting](../troubleshooting.md) (H.264/AAC).

## Picture in Picture

Choose **Picture in Picture** in a video's menu or in the fullscreen player. The video moves into a
small floating glass window that you can drag, resize (it keeps the video's shape) and that snaps to
the corners.

## Now Playing and media keys

While something plays, the sidebar shows a **Now Playing** card with the artwork, the title and
play controls. Axion also shows the track in your desktop's media widgets and reacts to your
keyboard's media keys (MPRIS). Turn this off with **Settings > Media > Desktop media controls and
media keys (after restart)**.

## Playlist

The Playlist saves videos for later and can keep copies for offline viewing.

### Add a video

- On a page with a video, press the **Playlist** button in the address field and choose **Add**. If
  the video is a plain file you can also choose **Add and Download**.
- In the fullscreen player, press the Playlist button (top left) to add the video that is playing.
- Or press **Ctrl+Shift+D**, or use **Add to Playlist** in the page menu.

If the video is already saved, the button offers **Open Playlist** and **Remove** instead. Private
tabs cannot add videos. The same video is never added twice.

If you do not want the button in the address field, turn it off with **Playlist button in the
address field** in **Settings > Media**.

### Watch

Open the Playlist with **Ctrl+Shift+P** or from the sidebar. The page lists your videos with
thumbnails. Pick one to play it in the player next to the list. You also get:

- play all, shuffle, previous and next, repeat (off, all or one)
- playback speed and a sleep timer
- fullscreen and Picture in Picture
- a search field that filters the list as you type
- **Edit** to select, remove, download or move videos

Only the video plays, never the website around it. Axion remembers where you stopped, resumes a
little earlier, and marks a video as watched when you reach the end. When a video ends, the next
one starts. Opening the Playlist starts the top video.

Some videos cannot be played outside their page (for example protected streams). The player says
so and offers to open the page in a normal tab.

### Download for offline

Videos that are plain files (for example MP4 or WebM) can be downloaded. Use **Add and Download**,
the **Download** action in a video's menu, or turn on **Download videos when adding them to the
Playlist**. Streaming formats (HLS and DASH) cannot be downloaded; they can be watched online only.

Downloaded files go to the **Playlist downloads** folder in **Settings > Media**. The default is
a folder inside your account. **Change…** picks another one (only new downloads are affected) and
**Default** goes back. With **Delete downloaded files when removing videos** on, removing a video
also deletes its file. A file that was your own is never deleted. **Remove Download** keeps the
video in the list but deletes the file.

### Sort and continue watching

**Playlist sort** in **Settings > Media** (also the sort button on the Playlist page) orders the list:

- **Newest First** (default)
- **Oldest First**
- **Custom Order** - the order you arranged with move up and move down in Edit mode

The video you watched last and did not finish stays at the very top, whatever the sort says
("continue watching"). Finished videos keep their sorted place.

## Live wallpapers

Animated images (GIF, WebP) and videos (WebM, MP4) can play as the background of the start page,
muted and looping. See [Start page](start-page.md).
