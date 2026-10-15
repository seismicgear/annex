-- Explicit channel membership contributes one graph hop to a virtual channel
-- vertex. Colleagues in that channel are degree two. This projection carries no
-- message/media data and never substitutes for channel authorization.
-- ':' cannot occur in a valid authenticated pseudonym, so a caller-selected
-- channel ID cannot impersonate a participant vertex.
INSERT OR IGNORE INTO graph_edges(server_id,from_node,to_node,kind,weight)
SELECT m.server_id,m.pseudonym_id,'annex:channel:' || m.server_id || ':' || m.channel_id,'MEMBER_OF',1.0
FROM channel_members m JOIN channels c ON c.server_id=m.server_id AND c.channel_id=m.channel_id;

-- Normalize existing application-created agent edges without changing other
-- relationship kinds. Capture row IDs first so even an adversarial channel ID
-- equal to another encoded vertex cannot cause newly inserted rows to be lost.
CREATE TEMP TABLE graph_063_agent_edges AS
SELECT e.id,e.server_id,e.from_node,'annex:channel:' || e.server_id || ':' || e.to_node AS target,e.weight,e.created_at
FROM graph_edges e JOIN channels c ON c.server_id=e.server_id AND c.channel_id=e.to_node
WHERE e.kind='AGENT_SERVING';
DELETE FROM graph_edges WHERE id IN (SELECT id FROM graph_063_agent_edges);
INSERT OR IGNORE INTO graph_edges(server_id,from_node,to_node,kind,weight,created_at)
SELECT server_id,from_node,target,'AGENT_SERVING',weight,created_at FROM graph_063_agent_edges;
DROP TABLE graph_063_agent_edges;

CREATE TRIGGER graph_member_insert AFTER INSERT ON channel_members
WHEN EXISTS(SELECT 1 FROM channels c WHERE c.server_id=NEW.server_id AND c.channel_id=NEW.channel_id)
BEGIN
    INSERT OR IGNORE INTO graph_edges(server_id,from_node,to_node,kind,weight)
    VALUES(NEW.server_id,NEW.pseudonym_id,'annex:channel:' || NEW.server_id || ':' || NEW.channel_id,'MEMBER_OF',1.0);
END;
CREATE TRIGGER graph_member_delete AFTER DELETE ON channel_members BEGIN
    DELETE FROM graph_edges WHERE server_id=OLD.server_id AND from_node=OLD.pseudonym_id
      AND to_node='annex:channel:' || OLD.server_id || ':' || OLD.channel_id
      AND kind IN ('MEMBER_OF','AGENT_SERVING');
END;
CREATE TRIGGER graph_member_update AFTER UPDATE OF server_id,channel_id,pseudonym_id ON channel_members BEGIN
    DELETE FROM graph_edges WHERE server_id=OLD.server_id AND from_node=OLD.pseudonym_id
      AND to_node='annex:channel:' || OLD.server_id || ':' || OLD.channel_id
      AND kind IN ('MEMBER_OF','AGENT_SERVING');
    INSERT OR IGNORE INTO graph_edges(server_id,from_node,to_node,kind,weight)
    SELECT NEW.server_id,NEW.pseudonym_id,'annex:channel:' || NEW.server_id || ':' || NEW.channel_id,'MEMBER_OF',1.0
    WHERE EXISTS(SELECT 1 FROM channels c WHERE c.server_id=NEW.server_id AND c.channel_id=NEW.channel_id);
END;
CREATE TRIGGER graph_channel_delete BEFORE DELETE ON channels BEGIN
    DELETE FROM graph_edges WHERE server_id=OLD.server_id
      AND to_node='annex:channel:' || OLD.server_id || ':' || OLD.channel_id
      AND kind IN ('MEMBER_OF','AGENT_SERVING');
END;
