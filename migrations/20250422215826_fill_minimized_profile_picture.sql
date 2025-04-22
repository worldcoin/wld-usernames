UPDATE names
SET minimized_profile_picture_url = 
  regexp_replace(profile_picture_url, 
                 '/([^/]+)$', 
                 '/minimized_\1')
WHERE profile_picture_url IS NOT NULL;