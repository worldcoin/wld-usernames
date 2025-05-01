UPDATE names
SET minimized_profile_picture_url = 
  left(profile_picture_url, length(profile_picture_url) - length(split_part(profile_picture_url, '/', -1))) ||
  'minimized_' || split_part(profile_picture_url, '/', -1)
WHERE profile_picture_url IS NOT NULL;