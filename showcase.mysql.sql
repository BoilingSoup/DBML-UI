CREATE TABLE `users` (
  `id` integer UNIQUE PRIMARY KEY AUTO_INCREMENT NOT NULL COMMENT 'The user identifier — hover the paper icon to read me',
  `username` varchar(80) UNIQUE NOT NULL COMMENT 'Login handle, unique across the instance',
  `email` varchar(255) UNIQUE NOT NULL,
  `display_name` varchar(120),
  `role` ENUM ('member', 'moderator', 'admin') NOT NULL DEFAULT 'member',
  `created_at` timestamp NOT NULL DEFAULT (now()),
  `referrer_id` integer COMMENT 'Self-reference: who invited this user'
);

CREATE TABLE `posts` (
  `id` integer PRIMARY KEY AUTO_INCREMENT NOT NULL,
  `author_id` integer NOT NULL COMMENT 'FK side of the link icon',
  `title` varchar(200) NOT NULL,
  `slug` varchar(220) UNIQUE NOT NULL,
  `body` text COMMENT 'Markdown body — can be very long, notes are capped at 60 chars in the tooltip',
  `status` ENUM ('draft', 'published', 'archived') NOT NULL DEFAULT 'draft',
  `published_at` timestamp
);

CREATE TABLE `comments` (
  `id` integer PRIMARY KEY AUTO_INCREMENT NOT NULL,
  `post_id` integer NOT NULL COMMENT 'Which post this comment belongs to',
  `author_id` integer NOT NULL,
  `body` text NOT NULL
);

CREATE TABLE `sessions` (
  `id` integer PRIMARY KEY AUTO_INCREMENT,
  `user_id` integer NOT NULL,
  `token` varchar(128) UNIQUE NOT NULL,
  `expires_at` timestamp NOT NULL
);

CREATE TABLE `audit_log` (
  `id` integer PRIMARY KEY AUTO_INCREMENT,
  `event` varchar(255),
  `masked_id` integer
);

CREATE TABLE `likes` (
  `id` integer PRIMARY KEY AUTO_INCREMENT,
  `user_id` integer NOT NULL,
  `post_id` integer NOT NULL
);

CREATE INDEX `idx_sessions_user` ON `sessions` (`user_id`);

CREATE INDEX `idx_sessions_user_expiry` ON `sessions` (`user_id`, `expires_at`) USING HASH;

ALTER TABLE `likes` ADD FOREIGN KEY (`user_id`) REFERENCES `users` (`id`);

ALTER TABLE `likes` ADD FOREIGN KEY (`post_id`) REFERENCES `posts` (`id`);

ALTER TABLE `posts` ADD FOREIGN KEY (`author_id`) REFERENCES `users` (`id`);

ALTER TABLE `comments` ADD FOREIGN KEY (`post_id`) REFERENCES `posts` (`id`);

ALTER TABLE `comments` ADD FOREIGN KEY (`author_id`) REFERENCES `users` (`id`);

ALTER TABLE `users` ADD FOREIGN KEY (`referrer_id`) REFERENCES `users` (`id`);

ALTER TABLE `sessions` ADD FOREIGN KEY (`user_id`) REFERENCES `users` (`id`);
