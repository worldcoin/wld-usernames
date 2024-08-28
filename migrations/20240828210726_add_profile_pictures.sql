-- Add profile picture support to the names table
ALTER TABLE names ADD COLUMN profile_picture_url VARCHAR(255);
