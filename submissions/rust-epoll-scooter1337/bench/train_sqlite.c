/* Build-time PGO trainer only; never linked into the Rust server. Uses a disposable
 * fixture and representative indexed reads/grouped writes. No HTTP/seed detection,
 * no application data retained. The production server always uses SQLITE_PATH. */
#include "sqlite3.h"
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <unistd.h>
static sqlite3 *db;
static void sql(const char *s) { char *e=0; if(sqlite3_exec(db,s,0,0,&e)) {fprintf(stderr,"PGO: %s\n",e);exit(1);} }
static sqlite3_stmt *prepare(const char *s) {sqlite3_stmt *st=0;if(sqlite3_prepare_v3(db,s,-1,SQLITE_PREPARE_PERSISTENT,&st,0))exit(2);return st;}
static void step(sqlite3_stmt *s) {
 int rc;
 while((rc=sqlite3_step(s))==SQLITE_ROW) {
  for(int i=0;i<sqlite3_column_count(s);i++) {
   if(sqlite3_column_type(s,i)==SQLITE_INTEGER) (void)sqlite3_column_int64(s,i);
   else { (void)sqlite3_column_text(s,i);(void)sqlite3_column_bytes(s,i); }
  }
 }
 if(rc!=SQLITE_DONE) {fprintf(stderr,"PGO step: %s\n",sqlite3_errmsg(db));exit(3);}
 sqlite3_reset(s);sqlite3_clear_bindings(s);
}
int main(int argc,char **argv) {
 if(argc!=2)return 1;unlink(argv[1]);
 if(sqlite3_open(argv[1],&db))return 2;
 sql("PRAGMA locking_mode=EXCLUSIVE;PRAGMA journal_mode=WAL;PRAGMA synchronous=NORMAL;PRAGMA foreign_keys=ON;PRAGMA cache_size=500;PRAGMA mmap_size=1073741824;"
 "CREATE TABLE users(id INTEGER PRIMARY KEY,username TEXT NOT NULL UNIQUE,created_at TEXT NOT NULL DEFAULT(strftime('%Y-%m-%dT%H:%M:%fZ','now')));"
 "CREATE TABLE posts(id INTEGER PRIMARY KEY,user_id INTEGER NOT NULL REFERENCES users(id),body TEXT NOT NULL CHECK(length(body) BETWEEN 1 AND 500),created_at TEXT NOT NULL DEFAULT(strftime('%Y-%m-%dT%H:%M:%fZ','now')));"
 "CREATE TABLE likes(user_id INTEGER NOT NULL REFERENCES users(id),post_id INTEGER NOT NULL REFERENCES posts(id),created_at TEXT NOT NULL DEFAULT(strftime('%Y-%m-%dT%H:%M:%fZ','now')),PRIMARY KEY(user_id,post_id));"
 "CREATE INDEX posts_created_at_id_idx ON posts(created_at DESC,id DESC);CREATE INDEX posts_user_id_idx ON posts(user_id);CREATE INDEX likes_post_id_idx ON likes(post_id);"
 "WITH RECURSIVE n(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1000) INSERT INTO users(id,username) SELECT x,printf('profile-user-%d',x) FROM n;"
 "WITH RECURSIVE n(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<10000) INSERT INTO posts(id,user_id,body) SELECT x,x%1000+1,printf('Representative feed text %d. SQL indexes, row lookups, counts and UTF-8 serialization are profiled during compilation.',x) FROM n;"
 "WITH RECURSIVE n(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<40000) INSERT INTO likes(user_id,post_id) SELECT (x/10000*17+x)%1000+1,x%10000+1 FROM n;ANALYZE;");
 const char *select="SELECT p.id,p.body,p.created_at,u.username,(SELECT count(*) FROM likes l WHERE l.post_id=p.id) FROM posts p JOIN users u ON u.id=p.user_id";
 char text[512];snprintf(text,sizeof text,"%s ORDER BY p.created_at DESC,p.id DESC LIMIT 20",select);sqlite3_stmt *feed=prepare(text);
 snprintf(text,sizeof text,"%s WHERE p.id=?1",select);sqlite3_stmt *post=prepare(text);
 sqlite3_stmt *like=prepare("INSERT INTO likes(user_id,post_id) SELECT ?1,?2 WHERE EXISTS(SELECT 1 FROM posts WHERE id=?2) ON CONFLICT(user_id,post_id) DO NOTHING");
 sqlite3_stmt *create=prepare("INSERT INTO posts(user_id,body) VALUES(?1,?2) RETURNING id,created_at");
 sqlite3_stmt *exists=prepare("SELECT 1 FROM posts WHERE id=?1"),*health=prepare("SELECT 1");
 uint32_t random=42;
 for(int i=0;i<100000;i++) {
  if(i%64==0)sql("BEGIN IMMEDIATE");
  random=random*1664525u+1013904223u;int r=random%217;int id=1+(random>>8)%10000;int user=1+(random>>16)%1000;
  if(r<100)step(feed);
  else if(r<200) {sqlite3_bind_int64(post,1,id);step(post);}
  else if(r<215) {
   sqlite3_bind_int64(like,1,user);sqlite3_bind_int64(like,2,id);step(like);
   if(!sqlite3_changes(db)) {sqlite3_bind_int64(exists,1,id);step(exists);}
  } else {sqlite3_bind_int64(create,1,user);sqlite3_bind_text(create,2,"A new profiled post",-1,SQLITE_STATIC);step(create);}
  if(i%64==63)sql("COMMIT");
 }
 sql("COMMIT");step(health);
 sqlite3_stmt *all[]={feed,post,like,create,exists,health};for(int i=0;i<6;i++)sqlite3_finalize(all[i]);
 sqlite3_close(db);unlink(argv[1]);return 0;
}
