UPDATE responsibilities SET plan_json=json_object(
    'trigger',json_object('kind','cron','expression',cron,'timeZone','UTC'),
    'action',json_object('kind','researchDigest','version',1,'config',
      json_object('taskId',research_task_id,'sources',json(sources_json))),
    'delivery',json_object('kind','feishu','recipient',feishu_recipient)
) WHERE research_task_id IS NOT NULL AND plan_json IS NULL;
