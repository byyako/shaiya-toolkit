# Loose data folders (a client without `data.sah`)

Some clients keep their data as plain files in folders instead of in `data.sah` and `data.saf`: an extracted client, or the `data` folder of a Shaiya-GX install. Shaiya Toolkit 0.5.0 reads such a folder as a client (read only: the writes arrive later). The folder is never converted and never changed.

## Which folder is it

A folder you choose is looked at in this order. A folder that holds `data.sah` (in any letter case) is an archive client and is never a loose one.

| # | The chosen folder | `data` folder | install root |
|---|---|---|---|
| 1 | is named `data` and holds at least two of `Character`, `Item`, `Monster`, `World`, `Effect` (any letter case) | the chosen folder | its parent |
| 2 | has a `data` folder inside that holds two of them | `data` inside it | the chosen folder |
| 3 | itself holds two of them (an extracted client) | the chosen folder | the chosen folder |
| - | none of these | not a loose client: the Add dialog says which folders were looked for | |

Tests: `detect_finds_the_three_layouts_and_the_gx_flag`, `a_folder_with_data_sah_is_not_loose`.

**Shaiya-GX layout.** The folder is called a Shaiya-GX layout (converted media) when it holds a `*.ogg` under `Sound/` or a `*_dds.png` file anywhere in the first four levels (a bounded look at no more than 20,000 entries; running out of entries means "no"). Evidence: read from Shaiya-GX (their data-format document: Ogg Vorbis audio, PNG interface images, `_dds.png` for an atlas that was DDS). The toolkit shows these files as they are: a PNG is previewed as a picture, an OGG shows its header line, nothing is converted. Test: `detect_finds_the_three_layouts_and_the_gx_flag`; on a folder made from US EP4.5 files the opt-in test `an_extracted_copy_of_us_client_files_reads_as_a_loose_client`. "Works with Shaiya-GX" is not claimed: no Shaiya-GX folder was opened.

## What is listed

The toolkit lists the files once when the client opens and reads each through its listed path. Names are looked up without regard to letter case (`/` or `\`), and a name is taken as it is on disk (`%XX` is not decoded).

| Left out of the list | Why | The toolkit says so |
|---|---|---|
| a symbolic link or junction inside the folder | it could lead out of the folder | yes; test `a_symlink_inside_the_tree_is_refused` |
| names that differ only in letter case (all spellings) | which one the game would use depends on the system | yes; test `case_only_duplicates_are_reported_and_left_out` |
| `*.shaiya-tmp` files, and the top-level `shaiya-toolkit-backups` folder | the toolkit's own files | the temporary files are counted; test `the_backup_folder_and_temps_are_not_listed` |
| names that are not text or hold `:` or `\`, paths over 1,024 bytes, folders deeper than 16 levels | they cannot be looked up safely | yes, counted; test `caps_refuse_or_leave_out_and_say_so` |

More than 1,000,000 files refuse the folder (test `caps_refuse_or_leave_out_and_say_so`). A file over 4 GiB is listed with its size and is never read. A folder that becomes a link after it was listed is not followed: the read is refused (test `a_link_that_appears_after_the_listing_refuses_the_read`). A cancel ends the listing and a read (test `a_cancelled_listing_and_read_end_cancelled`). The folder you choose may itself be a link; only links inside it are refused.

A file's stamp (what a cache compares to know a file is the one it learned about) is its modification time and its length, read from the disk each time, so a file changed on disk is learned again (tests `a_file_changed_on_disk_changes_its_stamp`, `a_loose_file_source_reads_and_its_stamp_follows_the_file`). A change that keeps both the time and the length is not seen.

## The backup folder

Writes to a loose client (a later release) need a backup folder that the person confirmed: never inside the `data` folder and never a folder that was not chosen. The Add dialog shows a proposal and the person confirms or changes it; it is saved as `backups`. For layouts 1 and 2 the proposal is `shaiya-toolkit-backups` in the install root (outside `data`); for layout 3 there is no proposal. Until a backup folder is saved, the client is read-only ("choose a backup folder outside the data folder"). A folder inside `data` or equal to it is refused. Nothing is written in this release. Tests: `the_proposed_backup_folder_follows_the_layout_rule`, `a_backup_folder_inside_data_is_refused`, `a_layout_3_client_is_read_only_until_a_backup_folder_is_chosen`.

## Where it is used

| Program | What works on a loose client |
|---|---|
| Shaiya Toolkit | Add (with the detected layout and the backup folder), the picker and Manage dialog (`loose`, `Shaiya-GX layout`), the Archive tab: browse, search, filters, preview of every kind, Extract (copy out), Export as JSON, glTF, OBJ and BVH, Export folder as JSON. Off: Compact, Save as new archive, Check, Use another archive pair (hover: "a loose folder has no archive"); Add, Replace, Remove, Apply, undo ("not yet for loose folders"); Health. Tests: `a_loose_client_browses_and_previews_like_an_archive`, `archive_only_actions_are_off_for_a_loose_client_with_the_reason`, `export_json_works_from_a_loose_client` |
| `shaiya` | `clients add`, `clients list`, `clients show`, `info FOLDER` and `info @name`, `anim ... --from @name` (also a folder). The archive commands (`archive`, `patch`, `clients health` and `clean`, `sdata --from`) refuse a loose client with exit 2. Tests: `from_a_loose_client_reads_the_file`, `archive_commands_refuse_a_loose_client_exit_2` |

An entry that an older Shaiya Toolkit saved for a loose folder is listed there as an archive client, and that version drops `kind`, `data` and `backups` when it next changes the list ([toolkit-files.md](toolkit-files.md)). Version 0.5.0 offers "Detect as loose folder" in Manage client folders for an entry that has no `kind`, no `data.sah` and a folder that passes the detection (test `an_entry_without_kind_that_detects_as_loose_is_offered_for_redetection`).
